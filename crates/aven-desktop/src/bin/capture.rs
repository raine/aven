//! Scene capture: seeds a throwaway database, renders both sides of each scene
//! and compares their boxes. Test support stays in this binary, out of the app.
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};
use aven_core::db::Database;
use aven_core::ids::TaskId;
use aven_desktop::adapter::{self, Snapshot};
use aven_desktop::assets::{Assets, load_fonts};
use aven_desktop::capabilities::{self, CapabilityMap, source_surfaces};
use aven_desktop::fixture::{self, Fixture, Seeded};
use aven_desktop::isolation::{self, Isolation};
use aven_desktop::measure::{self, MeasuredBounds, NamedBounds};
use aven_desktop::prototype::{Prototype, Render};
use aven_desktop::scene::{Manifest, Scene, Theme};
use aven_desktop::session::Session;
use aven_desktop::shell::Root;
use aven_desktop::{actions, input};
use aven_host::config::AppConfig;
use chrono::{DateTime, Local, TimeZone};
use gpui::{AnyWindowHandle, AppContext as _, Modifiers, VisualTestAppContext, point, px, size};
use serde_json::{Value, json};

const FIXTURE_INSTANT: (i32, u32, u32, u32) = (2026, 10, 14, 12);

fn crate_dir() -> &'static Path {
    Path::new(env!("CARGO_MANIFEST_DIR"))
}

fn repository_dir() -> PathBuf {
    crate_dir().join("../..")
}

fn manifest() -> Result<Manifest> {
    Manifest::load(&crate_dir().join("scenes/manifest.toml"))
}

/// A directory for a command that has no root of its own.
fn scratch() -> Result<tempfile::TempDir> {
    let base = std::env::temp_dir();
    isolation::require_outside_owner("TMPDIR", &base)?;
    Ok(tempfile::tempdir_in(base)?)
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match dispatch(&args) {
        Ok(true) => ExitCode::SUCCESS,
        Ok(false) => ExitCode::FAILURE,
        Err(error) => {
            eprintln!("capture failed: {error:#}");
            ExitCode::from(2)
        }
    }
}

fn dispatch(args: &[String]) -> Result<bool> {
    match args.iter().map(String::as_str).collect::<Vec<_>>()[..] {
        ["export-fixture"] => export_fixture().map(|()| true),
        ["seed"] => seed_only().map(|()| true),
        ["inventory"] => {
            let inventory = prototype_inventory()?;
            println!("{}", serde_json::to_string_pretty(&inventory)?);
            Ok(true)
        }
        ["check-capabilities"] => check_capabilities().map(|()| true),
        ["keychain-probe"] => keychain_probe().map(|()| true),
        ["list-bench", tasks] => list_bench(tasks.parse()?).map(|()| true),
        ["run", output] => run(Path::new(output), None),
        ["run", output, scene] => run(Path::new(output), Some(scene)),
        _ => bail!(
            "usage: aven-desktop-capture export-fixture | seed | inventory | check-capabilities | keychain-probe | list-bench TASKS | run OUTPUT [SCENE]"
        ),
    }
}

/// Regenerates `scenes/fixture.json` from the frozen prototype's own seed.
fn export_fixture() -> Result<()> {
    let (year, month, day, hour) = FIXTURE_INSTANT;
    let frozen = Local
        .with_ymd_and_hms(year, month, day, hour, 0, 0)
        .single()
        .context("fixture instant")?;
    let work = scratch()?;
    let prototype = Prototype::new(&repository_dir().join(manifest()?.target), work.path())?;
    let data = prototype.render(&Render {
        name: "export",
        now_ms: frozen.timestamp_millis(),
        width: 1440,
        height: 900,
        stored: None,
        steps: &[],
        result: "data",
        screenshot: None,
    })?;
    let fixture = Fixture {
        frozen_at: frozen.format("%Y-%m-%dT%H:%M").to_string(),
        data,
    };
    let path = crate_dir().join("scenes/fixture.json");
    std::fs::write(&path, serde_json::to_string_pretty(&fixture)? + "\n")?;
    println!("wrote {}", path.display());
    Ok(())
}

/// Every action the frozen prototype dispatches. Tables come from the running
/// page, and single keys, composer buttons and palette commands from its source.
fn prototype_inventory() -> Result<BTreeSet<String>> {
    let target = repository_dir().join(manifest()?.target);
    let html = std::fs::read_to_string(&target)?;
    let work = scratch()?;
    let tables = Prototype::new(&target, work.path())?.render(&Render {
        name: "inventory",
        now_ms: Local::now().timestamp_millis(),
        width: 1440,
        height: 900,
        stored: None,
        steps: &[],
        result: "[...Object.keys(ACT).map((name) => `act:${name}`), \
                 ...Object.entries(CHORDS).flatMap(([lead, keys]) => \
                   Object.keys(keys).map((key) => `chord:${lead} ${key}`))]",
        screenshot: None,
    })?;
    let mut inventory: BTreeSet<String> = serde_json::from_value(tables)?;
    let section = |start: &str, end: &str| -> Result<String> {
        let from = html.find(start).with_context(|| format!("find {start}"))?;
        let to = html[from..]
            .find(end)
            .with_context(|| format!("find {end}"))?;
        Ok(html[from..from + to].to_string())
    };
    let keyboard = section("document.addEventListener('keydown'", "const narrow")?;
    let palette = section("function palette()", "/* Navigation */")?;
    for (text, open, close, prefix) in [
        (html.as_str(), "data-c=\"", '"', "composer:"),
        (keyboard.as_str(), "k === '", '\'', "key:"),
        (palette.as_str(), "c('", '\'', "palette:"),
    ] {
        for (index, _) in text.match_indices(open) {
            let rest = &text[index + open.len()..];
            if let Some(end) = rest.find(close)
                && !rest[..end].contains('$')
            {
                inventory.insert(format!("{prefix}{}", &rest[..end]));
            }
        }
    }
    let script = &html[html.find("<script>").context("find the script")?..];
    inventory.extend(source_surfaces(
        script,
        "document.addEventListener('keydown'",
        "const narrow",
    ));
    Ok(inventory)
}

fn check_capabilities() -> Result<()> {
    let text = std::fs::read_to_string(crate_dir().join("capabilities.toml"))?;
    let map = CapabilityMap::parse(&text)?;
    map.check_coverage(&prototype_inventory()?)?;
    println!(
        "{} capabilities cover the prototype's action tables, keys, palette, dialog \
         controls, fields, handlers and data attributes; not inventoried: {}",
        map.capability.len(),
        capabilities::NOT_INVENTORIED
    );
    Ok(())
}

fn seed_database(isolation: &Isolation, anchor: DateTime<Local>) -> Result<Seeded> {
    isolation.create_directories()?;
    let fixture = Fixture::load(&crate_dir().join("scenes/fixture.json"))?;
    let shifted = fixture.shifted(anchor)?;
    let runtime = tokio::runtime::Runtime::new()?;
    runtime.block_on(async {
        let database = Database::open(&isolation.database).await?;
        let seeded = fixture::seed(&database, &shifted).await?;
        database.close().await;
        Ok(seeded)
    })
}

fn select_seeded_workspace(isolation: &Isolation, seeded: &Seeded) -> Result<()> {
    tokio::runtime::Runtime::new()?.block_on(async {
        let store = aven_core::api::Store::open(&isolation.database)
            .await
            .map_err(anyhow::Error::msg)?;
        store
            .select_queue_workspace(&seeded.workspace.id)
            .await
            .map_err(anyhow::Error::msg)?;
        store.close().await;
        anyhow::Ok(())
    })
}

fn seed_only() -> Result<()> {
    let isolation = Isolation::from_process()?;
    let seeded = seed_database(&isolation, fixture::anchor_now()?)?;
    select_seeded_workspace(&isolation, &seeded)?;
    println!("seeded {} tasks", seeded.ids.len());
    Ok(())
}

/// What the window shows at launch, read the way the app reads it.
fn load_state(isolation: &Isolation, now: DateTime<Local>) -> Result<Snapshot> {
    tokio::runtime::Runtime::new()?.block_on(async {
        let config = AppConfig::default();
        let store = adapter::open_store(&isolation.database, &config).await?;
        Ok(adapter::snapshot(&store, now))
    })
}

/// A view whose replies the caller feeds in, so a scene decides when they land.
fn open_root(
    cx: &mut VisualTestAppContext,
    session: Session,
    prefs: PathBuf,
    width: f32,
    height: f32,
) -> Result<gpui::WindowHandle<Root>> {
    cx.open_offscreen_window(size(px(width), px(height)), move |window, cx| {
        let (_, replies) = tokio::sync::mpsc::unbounded_channel();
        cx.new(|cx| {
            let mut root = Root::new(session, replies, prefs, window, cx);
            root.always_active = true;
            root
        })
    })
}

/// Times layout of a generated queue, and checks its extent matches its entries.
fn list_bench(tasks: usize) -> Result<()> {
    let work = scratch()?;
    let mut groups = Vec::new();
    let rows: Vec<_> = (0..tasks).collect();
    for (index, chunk) in rows.chunks(tasks.div_ceil(4).max(1)).enumerate() {
        groups.push(adapter::Group {
            name: format!("Group {index}"),
            hint: String::new(),
            rows: chunk.to_vec(),
            lane: None,
        });
    }
    let (requests, _) = tokio::sync::mpsc::unbounded_channel();
    let session = Session::new(adapter::bench_snapshot(tasks, groups), requests);
    let mut cx = capture_context();
    let started = Instant::now();
    let window = open_root(
        &mut cx,
        session,
        work.path().join("prefs.json"),
        1440.,
        900.,
    )?;
    let any: AnyWindowHandle = window.into();
    cx.run_until_parked();
    cx.update_window(any, |_, window, _| window.refresh())?;
    cx.run_until_parked();
    let first = started.elapsed();
    let frames = 20;
    let started = Instant::now();
    for _ in 0..frames {
        cx.update_window(any, |_, window, _| window.refresh())?;
        cx.run_until_parked();
    }
    let frame = started.elapsed() / frames;
    let started = Instant::now();
    window.update(&mut cx, |root, _, cx| {
        root.session_mut().snapshot.groups[0].rows.remove(0);
        root.reload_list(cx);
    })?;
    cx.update_window(any, |_, window, _| window.refresh())?;
    cx.run_until_parked();
    let changed = started.elapsed();
    let started = Instant::now();
    cx.simulate_keystrokes(any, "g d");
    cx.update_window(any, |_, window, _| window.refresh())?;
    cx.run_until_parked();
    let resized = started.elapsed();
    let scroll_max = cx.update(|cx| window.read(cx).map(|root| root.scroll_max()))?;
    let viewport = 900. - 6. * 2. - 44.;
    let sections: f32 = 4.;
    let expected = (tasks as f32 - 1.) * 31. + sections * 32.8 + 44. - viewport;
    println!(
        "{tasks} tasks: first {first:?}, frame {frame:?}, list change {changed:?}, \
         width change {resized:?}, max scroll {} expected {expected}",
        scroll_max
    );
    Ok(())
}

/// Leaves an isolated Chrome profile for `scripts/desktop/keychain-proof`.
fn keychain_probe() -> Result<()> {
    let isolation = Isolation::from_process()?;
    let profile = isolation
        .fresh_prototype_directory()?
        .join("keychain-probe");
    println!("{}", Prototype::write_keychain_probe(&profile)?);
    Ok(())
}

fn run(output: &Path, only: Option<&str>) -> Result<bool> {
    let isolation = Isolation::from_process()?;
    let manifest = manifest()?;
    let scenes: Vec<&Scene> = manifest
        .scene
        .iter()
        .filter(|scene| only.is_none_or(|name| scene.name == name))
        .collect();
    if scenes.is_empty() {
        bail!("no scene named {only:?}");
    }
    let output = &isolation::prepare_output(output)?;
    let anchor = fixture::anchor_now()?;
    let seeded = seed_database(&isolation, anchor)?;
    select_seeded_workspace(&isolation, &seeded)?;
    let state = load_state(&isolation, anchor)?;

    let prototype = Prototype::new(
        &repository_dir().join(&manifest.target),
        isolation.fresh_prototype_directory()?,
    )?;
    let mut cx = capture_context();
    let runtime = tokio::runtime::Runtime::new()?;
    let mut passed = true;
    let mut scene_reports = Vec::new();
    for scene in scenes {
        let page = render_prototype(&prototype, &manifest, scene, &seeded, anchor, output)?;
        let boxes: NamedBounds = serde_json::from_value(strip_missing(&page["boxes"]))?;
        let database = scene_database(&isolation, &scene.name)?;
        let launch = adapter::Launch {
            database,
            config: offline_config(),
            storage: isolation.protected_storage(),
        };
        let store = adapter::start(runtime.handle().clone(), launch)?;
        let gpui_boxes = render_gpui(&mut cx, scene, store, &seeded.ids, output)?;
        let present = unexpectedly_present(scene, &boxes, &gpui_boxes);
        let comparisons = measure::compare(&manifest.expectations(scene)?, &boxes, &gpui_boxes);
        // Steps change the prototype's data, and the seeded state is what is compared.
        let parity = if scene.prototype_steps.is_empty() {
            data_parity(&page["queue"], &state)
        } else {
            Vec::new()
        };
        let scene_passed = comparisons.iter().all(|result| result.passed)
            && parity.is_empty()
            && present.is_empty();
        passed &= scene_passed;
        for result in comparisons.iter().filter(|result| !result.passed) {
            println!(
                "FAIL {} {}: prototype {:?} gpui {:?} difference {:?} tolerance {}",
                scene.name,
                result.element,
                result.prototype,
                result.gpui,
                result.difference,
                result.tolerance
            );
        }
        for difference in &parity {
            println!("FAIL {} data: {difference}", scene.name);
        }
        for element in &present {
            println!("FAIL {} {element}", scene.name);
        }
        println!(
            "{} {}: {} boxes",
            if scene_passed { "ok  " } else { "FAIL" },
            scene.name,
            comparisons.len()
        );
        let report = json!({
            "scene": scene.name,
            "passed": scene_passed,
            "boxes": comparisons,
            "styles": page["styles"],
            "data_differences": parity,
            "unexpectedly_present": present,
        });
        std::fs::write(
            output.join(format!("{}.measure.json", scene.name)),
            serde_json::to_vec_pretty(&report)?,
        )?;
        scene_reports.push(json!({"scene": scene.name, "passed": scene_passed}));
    }
    // Scene stores still run on it, and their threads end with the process.
    std::mem::forget(runtime);
    std::fs::write(
        output.join("report.json"),
        serde_json::to_vec_pretty(&json!({
            "anchor": anchor.to_rfc3339(),
            "passed": passed,
            "scenes": scene_reports,
        }))?,
    )?;
    Ok(passed)
}

/// Elements the scene expects to be gone that either side still draws.
fn unexpectedly_present(scene: &Scene, prototype: &NamedBounds, gpui: &NamedBounds) -> Vec<String> {
    let drawn = |boxes: &NamedBounds, name: &String| {
        boxes
            .get(name)
            .is_some_and(|bounds| bounds.width > 0. && bounds.height > 0.)
    };
    scene
        .absent
        .iter()
        .flat_map(|name| {
            let mut sides = Vec::new();
            if drawn(prototype, name) {
                sides.push(format!("{name}: present in prototype"));
            }
            if drawn(gpui, name) {
                sides.push(format!("{name}: present in gpui"));
            }
            sides
        })
        .collect()
}

/// The page's boxes, without the `null`s of elements it did not render.
fn strip_missing(boxes: &Value) -> Value {
    let mut boxes = boxes.clone();
    if let Some(fields) = boxes.as_object_mut() {
        fields.retain(|_, value| !value.is_null());
    }
    boxes
}

fn render_prototype(
    prototype: &Prototype,
    manifest: &Manifest,
    scene: &Scene,
    seeded: &Seeded,
    anchor: DateTime<Local>,
    output: &Path,
) -> Result<Value> {
    let selectors: BTreeMap<_, _> = scene
        .measure
        .iter()
        .chain(&scene.absent)
        .map(|name| {
            let element = manifest.element(name)?;
            Ok((name.clone(), (element.selector.clone(), element.index)))
        })
        .collect::<Result<_>>()?;
    let theme = match scene.theme {
        Theme::Dark => "dark",
        Theme::Light => "paper",
    };
    let result = format!(
        r#"{{
          boxes: Object.fromEntries(Object.entries({selectors}).map(([name, [selector, index]]) => {{
            const node = document.querySelectorAll(selector)[index];
            if (!node) return [name, null];
            const box = node.getBoundingClientRect();
            return [name, {{ x: box.x, y: box.y, width: box.width, height: box.height }}];
          }})),
          styles: Object.fromEntries(Object.entries({selectors}).map(([name, [selector, index]]) => {{
            const node = document.querySelectorAll(selector)[index];
            if (!node) return [name, null];
            const style = getComputedStyle(node);
            return [name, {{ color: style.color, background: style.backgroundColor, fontSize: style.fontSize, fontWeight: style.fontWeight }}];
          }})),
          queue: [...document.querySelectorAll('.list .sect, .list .row')].map((node) => node.classList.contains('sect')
            ? {{ section: node.firstChild.textContent }}
            : {{ reference: node.querySelector('.ref').textContent, title: node.querySelector('.t').textContent }}),
        }}"#,
        selectors = serde_json::to_string(&selectors)?,
    );
    prototype.render(&Render {
        name: &scene.name,
        now_ms: anchor.timestamp_millis(),
        width: scene.width,
        height: scene.height,
        stored: Some(json!({ "data": seeded.prototype_data, "prefs": { "theme": theme } })),
        steps: &scene.prototype_steps,
        result: &result,
        screenshot: Some(&output.join(format!("{}.prototype.png", scene.name))),
    })
}

/// Differences in which tasks each group holds. Order within a group is left to
/// core, since data behaviour follows core rather than the simulated prototype.
fn data_parity(queue: &Value, snapshot: &Snapshot) -> Vec<String> {
    let mut page: BTreeMap<String, BTreeSet<(String, String)>> = BTreeMap::new();
    let mut section = String::new();
    for item in queue.as_array().into_iter().flatten() {
        if let Some(name) = item["section"].as_str() {
            section = name.to_string();
        } else {
            page.entry(section.clone()).or_default().insert((
                item["reference"].as_str().unwrap_or_default().to_string(),
                item["title"].as_str().unwrap_or_default().to_string(),
            ));
        }
    }
    let core: BTreeMap<String, BTreeSet<(String, String)>> = snapshot
        .groups
        .iter()
        .map(|group| {
            let rows = group
                .rows
                .iter()
                .map(|row| &snapshot.tasks[*row])
                .map(|row| (row.reference.clone(), row.title.clone()))
                .collect();
            (group.name.clone(), rows)
        })
        .collect();
    let names: BTreeSet<_> = page.keys().chain(core.keys()).cloned().collect();
    let mut differences = Vec::new();
    for name in names {
        let empty = BTreeSet::new();
        let left = page.get(&name).unwrap_or(&empty);
        let right = core.get(&name).unwrap_or(&empty);
        for (reference, title) in left.difference(right) {
            differences.push(format!("{name}: only in prototype: {reference} {title}"));
        }
        for (reference, title) in right.difference(left) {
            differences.push(format!("{name}: only in core: {reference} {title}"));
        }
    }
    differences
}

fn capture_context() -> VisualTestAppContext {
    let platform = gpui_platform::current_platform(false);
    let mut cx = VisualTestAppContext::with_asset_source(platform, Arc::new(Assets));
    cx.update(|cx| {
        load_fonts(cx);
        cx.set_global(aven_desktop::theme::Theme::dark());
        cx.bind_keys(input::key_bindings());
        cx.bind_keys(actions::key_bindings());
    });
    cx
}

/// A copy of the seeded database for one scene, so its writes end with it.
fn scene_database(isolation: &Isolation, name: &str) -> Result<PathBuf> {
    let directory = isolation.root.join("scene-data").join(name);
    if directory.exists() {
        std::fs::remove_dir_all(&directory)?;
    }
    std::fs::create_dir_all(&directory)?;
    let copy = directory.join("db.sqlite");
    std::fs::copy(&isolation.database, &copy).context("copy the seeded database")?;
    Ok(copy)
}

fn offline_config() -> AppConfig {
    let mut config = AppConfig::default();
    config.sync.disable_override = true;
    config
}

/// Hands replies to the view until the store has gone quiet.
fn settle(
    cx: &mut VisualTestAppContext,
    any: AnyWindowHandle,
    root: &gpui::Entity<Root>,
    replies: &mut adapter::Replies,
    quiet_polls: u32,
) -> Result<()> {
    let mut quiet = 0;
    while quiet < quiet_polls {
        let mut took = false;
        while let Ok(reply) = replies.try_recv() {
            took = true;
            cx.update_window(any, |_, window, cx| {
                root.update(cx, |root, cx| root.take_reply(reply, window, cx));
            })?;
        }
        cx.run_until_parked();
        if took {
            quiet = 0;
        } else {
            quiet += 1;
            std::thread::sleep(Duration::from_millis(10));
        }
    }
    Ok(())
}

/// A small gradient to stand for an attachment.
fn scene_image() -> adapter::PendingImage {
    let image = image::RgbaImage::from_fn(240, 160, |x, y| {
        image::Rgba([(x / 2) as u8, (y * 2) as u8, 160, 255])
    });
    let mut bytes = std::io::Cursor::new(Vec::new());
    image
        .write_to(&mut bytes, image::ImageFormat::Png)
        .expect("an image in memory encodes");
    adapter::PendingImage {
        name: "launch-plan.png".to_string(),
        bytes: std::sync::Arc::new(bytes.into_inner()),
    }
}

fn render_gpui(
    cx: &mut VisualTestAppContext,
    scene: &Scene,
    (session, mut replies): (Session, adapter::Replies),
    ids: &BTreeMap<String, TaskId>,
    output: &Path,
) -> Result<NamedBounds> {
    let theme = match scene.theme {
        Theme::Dark => aven_desktop::theme::Theme::dark(),
        Theme::Light => aven_desktop::theme::Theme::light(),
    };
    cx.update(|cx| {
        cx.set_global(theme);
        cx.set_global(MeasuredBounds::default());
    });
    let window = open_root(
        cx,
        session,
        output.join("capture-prefs.json"),
        scene.width as f32,
        scene.height as f32,
    )?;
    let window_root = window.entity(cx)?;
    let any: AnyWindowHandle = window.into();
    // Motion scenes are caught mid-flight, so they wait no longer than they must.
    let quiet_polls = if scene.gpui_frame_ms.is_some() { 2 } else { 12 };
    settle(cx, any, &window_root, &mut replies, 12)?;
    if let Some(task) = &scene.gpui_select {
        let id = ids[task].clone();
        cx.update_window(any, |_, window, cx| {
            window_root.update(cx, |root, cx| root.open_task(id, window, cx));
        })?;
        settle(cx, any, &window_root, &mut replies, 12)?;
    }
    if scene.gpui_image {
        cx.update(|cx| {
            window_root.update(cx, |root, _| {
                let session = root.session_mut();
                let id = session.target().cloned().expect("a task is open");
                session.write(adapter::Command::AddImage(id, scene_image()));
            });
        });
        // The worker stores the bytes off the test clock.
        for _ in 0..30 {
            settle(cx, any, &window_root, &mut replies, 4)?;
            std::thread::sleep(Duration::from_millis(20));
        }
    }
    for keys in &scene.gpui_keys {
        cx.simulate_keystrokes(any, keys);
        settle(cx, any, &window_root, &mut replies, quiet_polls)?;
    }
    if let Some(text) = &scene.gpui_incoming {
        cx.update(|cx| {
            window_root.update(cx, |root, _| {
                let session = root.session_mut();
                let id = session.target().cloned().expect("a task is open");
                session.write(adapter::Command::SetDescription(id, text.clone()));
            });
        });
        settle(cx, any, &window_root, &mut replies, 30)?;
    }
    if let Some([x, y]) = scene.gpui_pointer {
        cx.simulate_mouse_move(any, point(px(x), px(y)), None, Modifiers::default());
    }
    if let Some([x, y]) = scene.gpui_click {
        cx.simulate_click(any, point(px(x), px(y)), Modifiers::default());
        settle(cx, any, &window_root, &mut replies, quiet_polls)?;
    }
    if let Some(frame_ms) = scene.gpui_frame_ms {
        cx.update_window(any, |_, window, _| window.refresh())?;
        cx.run_until_parked();
        std::thread::sleep(Duration::from_millis(frame_ms));
    } else {
        for _ in 0..4 {
            cx.run_until_parked();
            cx.advance_clock(Duration::from_millis(50));
            cx.update_window(any, |_, window, _| window.refresh())?;
        }
        std::thread::sleep(Duration::from_millis(200));
    }
    cx.update_window(any, |_, window, _| window.refresh())?;
    cx.run_until_parked();
    let image = cx
        .capture_screenshot(any)
        .map_err(|error| anyhow::anyhow!("{error}"))?;
    image.save(output.join(format!("{}.gpui.png", scene.name)))?;
    let boxes = cx.update(|cx| cx.global::<MeasuredBounds>().0.clone());
    cx.update_window(any, |_, window, _| window.remove_window())?;
    cx.run_until_parked();
    Ok(boxes)
}

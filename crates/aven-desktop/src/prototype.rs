//! Renders the frozen prototype in headless Chrome.
//!
//! Each render uses a throwaway copy of the page and a throwaway browser
//! profile. Injected scripts freeze `Date`, load scene data into the page's
//! storage and return values through the DOM dump.
use std::fs::File;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};
use serde_json::Value;

const DEFAULT_CHROME: &str = "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome";
const STORAGE_KEY: &str = "aven-desktop-proto-v3-collab";
const TIMEOUT: Duration = Duration::from_secs(60);
const RESULT_OPEN: &str = "<script type=\"application/json\" id=\"harness-result\">";

pub struct Prototype {
    html: String,
    work: PathBuf,
}

pub struct Render<'a> {
    pub name: &'a str,
    pub now_ms: i64,
    pub width: u32,
    pub height: u32,
    /// The stored `{data, prefs}` document, or `None` for the page's own seed.
    pub stored: Option<Value>,
    /// JavaScript run after load, before `result`.
    pub steps: &'a [String],
    /// A JavaScript expression whose JSON value `render` returns.
    pub result: &'a str,
    pub screenshot: Option<&'a Path>,
}

impl Prototype {
    pub fn new(target: &Path, work: &Path) -> Result<Self> {
        let html = std::fs::read_to_string(target)
            .with_context(|| format!("read the frozen prototype {}", target.display()))?;
        std::fs::create_dir_all(work)?;
        Ok(Self {
            html,
            work: work.to_path_buf(),
        })
    }

    pub fn render(&self, render: &Render) -> Result<Value> {
        let page = self.work.join(format!("{}.html", render.name));
        std::fs::write(&page, self.instrument(render)?)?;
        // Chrome reserves window height for browser UI by mode, so the page
        // height is calibrated rather than assumed.
        let mut window_height = render.height;
        for _ in 0..3 {
            let (viewport, value) = self.dump(render, &page, window_height)?;
            if viewport == render.height {
                if let Some(screenshot) = render.screenshot {
                    self.screenshot(render, &page, screenshot)?;
                }
                return Ok(value);
            }
            window_height += render.height.saturating_sub(viewport);
        }
        bail!("{}: Chrome did not reach the scene height", render.name)
    }

    fn instrument(&self, render: &Render) -> Result<String> {
        let stored = match &render.stored {
            Some(stored) => format!(
                "localStorage.setItem({key}, {value});",
                key = serde_json::to_string(STORAGE_KEY)?,
                value = script_json(&serde_json::to_string(&stored.to_string())?),
            ),
            None => format!(
                "localStorage.removeItem({});",
                serde_json::to_string(STORAGE_KEY)?
            ),
        };
        let head = format!(
            r#"<style>*,*::before,*::after{{animation:none!important;transition:none!important;caret-color:transparent!important}}</style>
<script>(()=>{{const now={now};const Real=Date;class Frozen extends Real{{constructor(...args){{if(args.length)super(...args);else super(now);}}static now(){{return now;}}}}window.Date=Frozen;{stored}}})();</script>
"#,
            now = render.now_ms,
        );
        let steps = render.steps.join(";\n");
        let tail = format!(
            r#"<script>setTimeout(()=>{{let out;try{{{steps};out={{viewport:innerHeight,value:({result})}};}}catch(error){{out={{error:String(error)}};}}
const node=document.createElement('script');node.type='application/json';node.id='harness-result';node.textContent=JSON.stringify(out).replaceAll('<','\\u003c');document.documentElement.append(node);}},200);</script>
"#,
            result = render.result,
        );
        // Remote fonts would make a render depend on the network.
        let html = self
            .html
            .lines()
            .filter(|line| !line.contains("fonts.googleapis.com"))
            .collect::<Vec<_>>()
            .join("\n");
        let Some(script) = html.find("<script>") else {
            bail!("the prototype has no script element");
        };
        let Some(end) = html.rfind("</body>") else {
            bail!("the prototype has no closing body");
        };
        Ok(format!(
            "{}{head}{}{tail}{}",
            &html[..script],
            &html[script..end],
            &html[end..]
        ))
    }

    fn chrome(&self, render: &Render, window_height: u32, profile: &str) -> Command {
        let chrome = std::env::var("AVEN_DESKTOP_CHROME").unwrap_or_else(|_| DEFAULT_CHROME.into());
        let mut command = Command::new(chrome);
        command
            .args([
                "--headless=new",
                "--disable-gpu",
                "--hide-scrollbars",
                "--no-first-run",
                "--no-default-browser-check",
                "--force-device-scale-factor=2",
                "--force-color-profile=srgb",
                "--virtual-time-budget=3000",
            ])
            .arg(format!(
                "--user-data-dir={}",
                self.work.join(profile).display()
            ))
            .arg(format!("--window-size={},{window_height}", render.width));
        command
    }

    fn dump(&self, render: &Render, page: &Path, window_height: u32) -> Result<(u32, Value)> {
        let output = self.work.join(format!("{}.dom", render.name));
        let mut command = self.chrome(render, window_height, &format!("{}-dump", render.name));
        command
            .arg("--dump-dom")
            .arg(format!("file://{}", page.display()))
            .stdout(File::create(&output)?)
            .stderr(Stdio::null());
        finish(command.spawn().context("start Chrome")?)?;
        let dom = std::fs::read_to_string(&output)?;
        let start = dom
            .find(RESULT_OPEN)
            .with_context(|| format!("{}: the page returned no result", render.name))?
            + RESULT_OPEN.len();
        let end = dom[start..]
            .find("</script>")
            .context("unterminated result")?
            + start;
        let result: Value = serde_json::from_str(&dom[start..end]).context("parse page result")?;
        if let Some(error) = result.get("error") {
            bail!("{}: {error}", render.name);
        }
        let viewport = result["viewport"].as_u64().context("page viewport")? as u32;
        Ok((viewport, result["value"].clone()))
    }

    fn screenshot(&self, render: &Render, page: &Path, path: &Path) -> Result<()> {
        // Screenshot mode sizes the viewport to the window exactly.
        let mut command = self.chrome(render, render.height, &format!("{}-shot", render.name));
        command
            .arg(format!("--screenshot={}", path.display()))
            .arg(format!("file://{}", page.display()))
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        finish(command.spawn().context("start Chrome")?)?;
        if !path.exists() {
            bail!("{}: Chrome wrote no screenshot", render.name);
        }
        Ok(())
    }
}

/// A JSON string literal is valid JavaScript once `</` cannot end the script.
fn script_json(json: &str) -> String {
    json.replace("</", "<\\/")
}

/// Waits for Chrome. Its teardown can exit non-zero after a good render, so
/// callers judge the output rather than the status.
fn finish(mut child: Child) -> Result<()> {
    let started = Instant::now();
    loop {
        if child.try_wait()?.is_some() {
            return Ok(());
        }
        if started.elapsed() > TIMEOUT {
            child.kill().ok();
            child.wait().ok();
            bail!("Chrome timed out");
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}

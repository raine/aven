//! Renders the frozen prototype in headless Chrome.
//!
//! Each render uses a throwaway copy of the page and a throwaway browser
//! profile. Injected scripts freeze `Date`, load scene data into the page's
//! storage and publish a result. One DevTools session over a pipe sets the exact
//! viewport, reads the result and takes the screenshot, so the image shows the
//! page state the result describes.
use std::fs::File;
use std::io::{BufRead, BufReader, Write};
use std::os::fd::{AsRawFd, OwnedFd};
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};
use base64::Engine;
use serde_json::{Value, json};

const DEFAULT_CHROME: &str = "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome";
const STORAGE_KEY: &str = "aven-desktop-proto-v3-collab";
const SCALE: u32 = 2;
const TIMEOUT: Duration = Duration::from_secs(60);
const POLL: Duration = Duration::from_millis(50);
/// Waits for two frames, so everything the page rendered has been painted.
const PAINTED: &str =
    "new Promise((done) => requestAnimationFrame(() => requestAnimationFrame(done)))";

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
        let mut session = Session::start(&self.work.join(render.name))?;
        let result = session.render(render, &page);
        session.close();
        result
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
            r#"<script>setTimeout(()=>{{let out;try{{{steps};out={{value:({result})}};}}catch(error){{out={{error:String(error)}};}}
window.harnessResult=JSON.stringify(out);}},200);</script>
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
}

/// A headless Chrome driven through its DevTools pipe.
struct Session {
    child: Child,
    to_chrome: File,
    from_chrome: BufReader<File>,
    next_id: u64,
    page: Option<String>,
    /// Dropping this ends the watchdog.
    _watchdog: mpsc::Sender<()>,
}

impl Session {
    fn start(profile: &Path) -> Result<Self> {
        let chrome = std::env::var("AVEN_DESKTOP_CHROME").unwrap_or_else(|_| DEFAULT_CHROME.into());
        let (chrome_reads, to_chrome) = std::io::pipe()?;
        let (from_chrome, chrome_writes) = std::io::pipe()?;
        let (read_fd, write_fd) = (chrome_reads.as_raw_fd(), chrome_writes.as_raw_fd());
        let mut command = Command::new(chrome);
        command
            .args([
                "--headless=new",
                "--disable-gpu",
                "--hide-scrollbars",
                "--no-first-run",
                "--no-default-browser-check",
                "--force-color-profile=srgb",
                "--remote-debugging-pipe",
            ])
            .arg(format!("--user-data-dir={}", profile.display()))
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        // SAFETY: only async-signal-safe calls run between fork and exec. Chrome
        // reads commands from descriptor 3 and writes responses to 4.
        unsafe {
            command.pre_exec(move || {
                let read = libc::fcntl(read_fd, libc::F_DUPFD, 10);
                let write = libc::fcntl(write_fd, libc::F_DUPFD, 10);
                if read < 0 || write < 0 || libc::dup2(read, 3) < 0 || libc::dup2(write, 4) < 0 {
                    return Err(std::io::Error::last_os_error());
                }
                libc::close(read);
                libc::close(write);
                Ok(())
            });
        }
        let child = command.spawn().context("start Chrome")?;
        drop((chrome_reads, chrome_writes));

        let pid = child.id() as i32;
        let (watchdog, expired) = mpsc::channel::<()>();
        std::thread::spawn(move || {
            if expired.recv_timeout(TIMEOUT) == Err(mpsc::RecvTimeoutError::Timeout) {
                // SAFETY: `kill` takes plain integers. Closing the pipe ends
                // any pending read.
                unsafe { libc::kill(pid, libc::SIGKILL) };
            }
        });
        Ok(Self {
            child,
            to_chrome: File::from(OwnedFd::from(to_chrome)),
            from_chrome: BufReader::new(File::from(OwnedFd::from(from_chrome))),
            next_id: 0,
            page: None,
            _watchdog: watchdog,
        })
    }

    fn call(&mut self, method: &str, params: Value) -> Result<Value> {
        self.next_id += 1;
        let mut message = json!({ "id": self.next_id, "method": method, "params": params });
        if let Some(page) = &self.page {
            message["sessionId"] = json!(page);
        }
        self.to_chrome.write_all(message.to_string().as_bytes())?;
        self.to_chrome.write_all(&[0])?;
        loop {
            let mut bytes = Vec::new();
            self.from_chrome.read_until(0, &mut bytes)?;
            if bytes.pop() != Some(0) {
                bail!("Chrome closed during {method}");
            }
            let reply: Value = serde_json::from_slice(&bytes)?;
            if reply["id"] != self.next_id {
                continue;
            }
            if !reply["error"].is_null() {
                bail!("{method}: {}", reply["error"]);
            }
            return Ok(reply["result"].clone());
        }
    }

    fn evaluate(&mut self, expression: &str) -> Result<Value> {
        let reply = self.call(
            "Runtime.evaluate",
            json!({ "expression": expression, "awaitPromise": true, "returnByValue": true }),
        )?;
        if !reply["exceptionDetails"].is_null() {
            bail!("page script: {}", reply["exceptionDetails"]);
        }
        Ok(reply["result"]["value"].clone())
    }

    fn render(&mut self, render: &Render, page: &Path) -> Result<Value> {
        let target = self.call("Target.createTarget", json!({ "url": "about:blank" }))?;
        let attached = self.call(
            "Target.attachToTarget",
            json!({ "targetId": target["targetId"], "flatten": true }),
        )?;
        self.page = attached["sessionId"].as_str().map(str::to_string);
        self.call(
            "Emulation.setDeviceMetricsOverride",
            json!({
                "width": render.width,
                "height": render.height,
                "deviceScaleFactor": SCALE,
                "mobile": false,
            }),
        )?;
        self.call(
            "Page.navigate",
            json!({ "url": format!("file://{}", page.display()) }),
        )?;
        let text = loop {
            match self.evaluate("window.harnessResult")?.as_str() {
                Some(text) => break text.to_string(),
                None => std::thread::sleep(POLL),
            }
        };
        let result: Value = serde_json::from_str(&text).context("parse page result")?;
        if let Some(error) = result.get("error") {
            bail!("{}: {error}", render.name);
        }
        if let Some(path) = render.screenshot {
            self.evaluate(PAINTED)?;
            let shot = self.call("Page.captureScreenshot", json!({ "format": "png" }))?;
            let data = shot["data"].as_str().context("screenshot data")?;
            let image = base64::engine::general_purpose::STANDARD.decode(data)?;
            std::fs::write(path, image)?;
            check_image(render, path)?;
        }
        Ok(result["value"].clone())
    }

    fn close(mut self) {
        self.page = None;
        self.call("Browser.close", json!({})).ok();
        let started = Instant::now();
        while started.elapsed() < Duration::from_secs(5) {
            if !matches!(self.child.try_wait(), Ok(None)) {
                return;
            }
            std::thread::sleep(POLL);
        }
        self.child.kill().ok();
        self.child.wait().ok();
    }
}

/// The image must have the scene's exact size at the capture scale.
fn check_image(render: &Render, path: &Path) -> Result<()> {
    let size = image::image_dimensions(path)
        .with_context(|| format!("{}: read the screenshot", render.name))?;
    let expected = (render.width * SCALE, render.height * SCALE);
    if size != expected {
        bail!(
            "{}: screenshot is {size:?}, expected {expected:?}",
            render.name
        );
    }
    Ok(())
}

/// A JSON string literal is valid JavaScript once `</` cannot end the script.
fn script_json(json: &str) -> String {
    json.replace("</", "<\\/")
}

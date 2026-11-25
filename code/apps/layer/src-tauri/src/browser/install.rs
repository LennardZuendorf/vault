//! The app's own Chrome (BRW-24): Chrome for Testing, downloaded from
//! Google into the Caches folder, a folder per version, with a newer one
//! fetched in the background.
//!
//! Not a Chrome installed on the Mac (BRW-1). macOS took one run from
//! Google Chrome.app for the user's own Chrome, which then would not open
//! from the Dock; and asking people to install a second Chrome by hand was
//! a step nobody should need, which most never took.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime};

use parking_lot::Mutex;
use serde::Serialize;
use serde_json::Value;
use tauri::{AppHandle, Manager, Runtime};
use tokio::io::{AsyncSeekExt, AsyncWriteExt};

use crate::commands::{off_runtime, AppState};
use crate::error::{Error, Result};

/// Google's list of the current Chrome for Testing in each channel, with
/// where each build is downloaded from.
const VERSIONS: &str =
    "https://googlechromelabs.github.io/chrome-for-testing/last-known-good-versions-with-downloads.json";

/// The only place a build is downloaded from. A list naming another place
/// would otherwise decide what the app runs.
const STORAGE: &str = "https://storage.googleapis.com/chrome-for-testing-public/";

/// The app inside a version's folder, and its executable inside the app.
const APP: &str = "Google Chrome for Testing.app";
const EXECUTABLE: &str = "Contents/MacOS/Google Chrome for Testing";

/// How long the app goes without asking Google for a newer version.
const CHECK_EVERY: Duration = Duration::from_secs(24 * 60 * 60);

/// A download left by a copy of the app that quit or crashed partway is
/// removed after this. Not sooner: another profile's copy of the app may be
/// downloading into it right now.
const STALE_PART: Duration = Duration::from_secs(24 * 60 * 60);

#[cfg(target_arch = "aarch64")]
const PLATFORM: &str = "mac-arm64";
#[cfg(not(target_arch = "aarch64"))]
const PLATFORM: &str = "mac-x64";

/// The download the browser waits on, as the panel shows it.
#[derive(Debug, Clone, Copy, Default, Serialize, PartialEq)]
pub struct Download {
    pub done: u64,
    /// 0 until Google says.
    pub total: u64,
}

#[derive(Default)]
pub struct Install {
    /// Held for as long as a download runs, by the download itself: a
    /// caller that goes away (an agent's call that timed out) leaves it
    /// running, and the next caller waits for it instead of starting
    /// another one from the beginning.
    busy: Arc<tokio::sync::Mutex<()>>,
    /// The download the browser waits on, while it runs.
    progress: Mutex<Option<Download>>,
    /// Why the last download the browser waited on failed, until the next.
    failed: Mutex<Option<String>>,
    /// When this process last asked Google for a newer version.
    checked: Mutex<Option<Instant>>,
    /// A Chrome a test runs instead.
    pinned: Mutex<Option<PathBuf>>,
}

impl super::Browser {
    /// The download the browser waits on, and why the last one failed.
    pub fn download(&self) -> (Option<Download>, Option<String>) {
        (*self.install.progress.lock(), self.install.failed.lock().clone())
    }
}

impl Install {
    #[cfg(test)]
    pub(super) fn pin(&self, chrome: PathBuf) {
        *self.pinned.lock() = Some(chrome);
    }

    /// Whether it is time to ask Google for a newer version, and if so,
    /// that it has been asked: two browser starts at once ask once.
    fn due(&self) -> bool {
        let mut checked = self.checked.lock();
        if checked.is_some_and(|at| at.elapsed() < CHECK_EVERY) {
            return false;
        }
        *checked = Some(Instant::now());
        true
    }
}

/// The Chrome to run: the newest version downloaded, or one downloaded now
/// and waited for when there is none (BRW-24). A newer version is looked
/// for in the background, at most once a day, and run from the next start.
pub async fn chrome<R: Runtime>(app: &AppHandle<R>) -> Result<PathBuf> {
    let install = &app.state::<AppState>().inner().browser.install;
    if let Some(pinned) = install.pinned.lock().clone() {
        return Ok(pinned);
    }
    let folder = folder(app)?;
    let looking = folder.clone();
    if let Some((_, exe)) = off_runtime(move || newest(&looking)).await? {
        if install.due() {
            let app = app.clone();
            tauri::async_runtime::spawn(async move {
                if let Err(e) = fetch(app, folder, false).await {
                    eprintln!("the browser's update was not downloaded: {e}");
                }
            });
        }
        return Ok(exe);
    }
    let app = app.clone();
    tauri::async_runtime::spawn(async move { fetch(app, folder, true).await })
        .await
        .map_err(|e| Error::Other(format!("the browser's download stopped: {e}")))?
}

/// Where the app keeps its Chrome: the Caches folder, as it can always be
/// downloaded again, and shared by every profile, which are all this
/// identifier.
fn folder<R: Runtime>(app: &AppHandle<R>) -> Result<PathBuf> {
    let caches = app.path().app_cache_dir().map_err(|e| Error::Other(format!("no Caches folder: {e}")))?;
    Ok(caches.join("chrome"))
}

/// Download the current stable version, unless one as new is there already.
/// `waited`: the browser waits on it, so the panel shows it, and there being
/// any version at all (another caller's download, finished meanwhile) will do.
async fn fetch<R: Runtime>(app: AppHandle<R>, folder: PathBuf, waited: bool) -> Result<PathBuf> {
    let install = &app.state::<AppState>().inner().browser.install;
    let _busy = install.busy.clone().lock_owned().await;
    let looking = folder.clone();
    let have = off_runtime(move || newest(&looking)).await?;
    if let (true, Some((_, exe))) = (waited, &have) {
        return Ok(exe.clone());
    }
    if waited {
        *install.failed.lock() = None;
        *install.progress.lock() = Some(Download::default());
        // What it downloads is the current version: no need to ask again
        // at the next start.
        *install.checked.lock() = Some(Instant::now());
        super::changed(&app, "");
    }
    let got = download(&app, &folder, have, waited).await;
    if waited {
        *install.progress.lock() = None;
        if let Err(e) = &got {
            *install.failed.lock() = Some(e.to_string());
        }
        super::changed(&app, "");
    }
    let (version, exe) = got.map_err(|e| Error::Other(format!("The browser could not be downloaded: {e}")))?;
    off_runtime(move || clear_old(&folder, &version, &running())).await?;
    Ok(exe)
}

/// Ask Google for the current stable build and, when it is newer than
/// `have`, download and unpack it. The version and its executable.
async fn download<R: Runtime>(
    app: &AppHandle<R>,
    folder: &Path,
    have: Option<(Vec<u32>, PathBuf)>,
    waited: bool,
) -> Result<(String, PathBuf)> {
    let list: Value = crate::integrations::http_client().get(VERSIONS).send().await?.error_for_status()?.json().await?;
    let build = stable(&list, PLATFORM)?;
    if let Some((version, exe)) = have.filter(|(v, _)| *v >= build.parts) {
        let version = version.iter().map(u32::to_string).collect::<Vec<_>>().join(".");
        return Ok((version, exe));
    }

    let part = folder.join(format!(".part-{}", uuid::Uuid::new_v4()));
    let making = part.clone();
    off_runtime(move || std::fs::create_dir_all(&making)).await??;
    let zip = part.join("chrome.zip");
    let got = async {
        save(app, &build.url, &zip, waited).await?;
        let (part, folder, version) = (part.clone(), folder.to_path_buf(), build.version.clone());
        off_runtime(move || unpack(&zip, &part, &version, &folder)).await?
    }
    .await;
    let leftover = part.clone();
    let _ = off_runtime(move || std::fs::remove_dir_all(leftover)).await?;
    Ok((build.version, got?))
}

/// How many times in a row a download may break off without a byte more
/// before it fails.
const TRIES: u32 = 5;

/// Stream `url` into `to`, saying how much has come while the browser waits.
/// A connection that drops is asked for the rest (a Range request): on a
/// line that reset every few minutes, 200 MB never came in one piece.
async fn save<R: Runtime>(app: &AppHandle<R>, url: &str, to: &Path, waited: bool) -> Result<()> {
    // Not the shared client: its 30 seconds are for an API's answer, and
    // 200 MB on a slow line take minutes. A stall still ends it.
    let client = reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(10))
        .read_timeout(Duration::from_secs(30))
        .user_agent("villain-layer")
        .build()?;
    let mut file = tokio::fs::File::create(to).await?;
    let mut got = Got::default();
    let mut idle = 0;
    loop {
        let before = got.done;
        let Some(broke) = receive(app, &client, url, &mut file, &mut got, waited).await? else { break };
        idle = if got.done > before { 1 } else { idle + 1 };
        if idle >= TRIES {
            return Err(broken(broke));
        }
        tokio::time::sleep(Duration::from_secs(2)).await;
    }
    file.flush().await?;
    if got.total != 0 && got.done != got.total {
        return Err(Error::Other(format!("the download stopped at {} of {} bytes", got.done, got.total)));
    }
    Ok(())
}

/// How far a download has come, and how much of it the panel was told.
#[derive(Default)]
struct Got {
    done: u64,
    /// 0 until the server says.
    total: u64,
    told: u64,
}

/// One request for what has not come yet, written after what has. Some: the
/// connection broke, and asking again may bring the rest.
async fn receive<R: Runtime>(
    app: &AppHandle<R>,
    client: &reqwest::Client,
    url: &str,
    file: &mut tokio::fs::File,
    got: &mut Got,
    waited: bool,
) -> Result<Option<reqwest::Error>> {
    let mut ask = client.get(url);
    if got.done > 0 {
        ask = ask.header(reqwest::header::RANGE, format!("bytes={}-", got.done));
    }
    let mut res = match ask.send().await.and_then(|r| r.error_for_status()) {
        Ok(res) => res,
        Err(e) if again(&e) => return Ok(Some(e)),
        Err(e) => return Err(broken(e)),
    };
    if res.status() != reqwest::StatusCode::PARTIAL_CONTENT {
        // The whole file again, not the rest: written from the start.
        if got.done > 0 {
            file.set_len(0).await?;
            file.seek(std::io::SeekFrom::Start(0)).await?;
        }
        *got = Got { total: res.content_length().unwrap_or(0), ..Got::default() };
    }
    let install = &app.state::<AppState>().inner().browser.install;
    // About a hundred updates in all, and at least one per megabyte.
    let step = (got.total / 100).clamp(1 << 20, 4 << 20);
    loop {
        let chunk = match res.chunk().await {
            Ok(Some(chunk)) => chunk,
            Ok(None) => return Ok(None),
            Err(e) if again(&e) => return Ok(Some(e)),
            Err(e) => return Err(broken(e)),
        };
        file.write_all(&chunk).await?;
        got.done += chunk.len() as u64;
        if waited && got.done - got.told >= step {
            got.told = got.done;
            *install.progress.lock() = Some(Download { done: got.done, total: got.total });
            super::changed(app, "");
        }
    }
}

/// A connection that failed or broke off, which asking again may mend; not
/// an answer like 404, which it would not.
fn again(e: &reqwest::Error) -> bool {
    e.is_connect() || e.is_timeout() || e.is_request() || e.is_body() || e.is_decode()
}

/// A failed download, with the cause under reqwest's own words: "error
/// decoding response body" alone was all a dropped connection said.
fn broken(e: reqwest::Error) -> Error {
    use std::error::Error as _;
    let mut text = e.to_string();
    let mut under = e.source();
    while let Some(cause) = under {
        text.push_str(&format!(": {cause}"));
        under = cause.source();
    }
    Error::Other(text)
}

/// A build in Google's list: its version, as text and as numbers, and where
/// to download it.
#[derive(Debug, PartialEq)]
struct Build {
    version: String,
    parts: Vec<u32>,
    url: String,
}

/// The stable channel's build for `platform`, from Google's list. Its
/// version names a folder and its address is downloaded, so both are
/// checked: numbers and dots only, and Google's own storage only.
fn stable(list: &Value, platform: &str) -> Result<Build> {
    let channel = &list["channels"]["Stable"];
    let version = channel["version"].as_str().unwrap_or_default().to_string();
    let parts = version_parts(&version)
        .ok_or_else(|| Error::Other(format!("Google's list names an odd version: {version:?}")))?;
    let url = channel["downloads"]["chrome"]
        .as_array()
        .into_iter()
        .flatten()
        .find(|d| d["platform"].as_str() == Some(platform))
        .and_then(|d| d["url"].as_str())
        .ok_or_else(|| Error::Other(format!("Google's list has no Chrome for {platform}")))?;
    if !url.starts_with(STORAGE) {
        return Err(Error::Other(format!("Google's list points somewhere else for Chrome: {url}")));
    }
    Ok(Build { version, parts, url: url.to_string() })
}

/// "158.0.8127.88" as numbers, compared as numbers: 158.0.10 is newer than
/// 158.0.9. None for anything else, `..` included.
fn version_parts(version: &str) -> Option<Vec<u32>> {
    if version.is_empty() {
        return None;
    }
    version.split('.').map(|p| if p.is_empty() { None } else { p.parse().ok() }).collect()
}

/// The newest version unpacked in `folder`, and its executable. A folder
/// without one (unpacked by hand, or by something else) is passed over.
pub(super) fn newest(folder: &Path) -> Option<(Vec<u32>, PathBuf)> {
    std::fs::read_dir(folder)
        .ok()?
        .flatten()
        .filter_map(|e| {
            let parts = version_parts(e.file_name().to_str()?)?;
            let exe = e.path().join(APP).join(EXECUTABLE);
            exe.is_file().then_some((parts, exe))
        })
        .max_by(|a, b| a.0.cmp(&b.0))
}

/// Unpack the downloaded zip and move the app into its version's folder.
/// The executable. Blocking.
fn unpack(zip: &Path, part: &Path, version: &str, folder: &Path) -> Result<PathBuf> {
    // ditto, as Finder unpacks: the app's frameworks are symlinks, and its
    // helpers must stay executable.
    let out = Command::new("/usr/bin/ditto").arg("-x").arg("-k").arg(zip).arg(part).output()?;
    if !out.status.success() {
        return Err(Error::Other(format!(
            "the browser's download did not unpack: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        )));
    }
    let unpacked = part.join(format!("chrome-{PLATFORM}"));
    let says = version_of(&unpacked.join(APP).join(EXECUTABLE))?;
    if !says.split_whitespace().any(|w| w == version) {
        return Err(Error::Other(format!("the downloaded browser says it is {:?}, not {version}", says.trim())));
    }
    let to = folder.join(version);
    match std::fs::rename(&unpacked, &to) {
        Ok(()) => {}
        // Another profile's copy of the app unpacked the same version first.
        Err(_) if to.join(APP).join(EXECUTABLE).is_file() => {}
        Err(e) => return Err(e.into()),
    }
    Ok(to.join(APP).join(EXECUTABLE))
}

/// What `chrome --version` prints: that it runs on this Mac at all, and
/// which version it is. Ten seconds at the most. Blocking.
fn version_of(exe: &Path) -> Result<String> {
    let mut child = Command::new(exe)
        .arg("--version")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| Error::Other(format!("the downloaded browser does not run: {e}")))?;
    let deadline = Instant::now() + Duration::from_secs(10);
    while child.try_wait()?.is_none() {
        if Instant::now() > deadline {
            let _ = child.kill();
            let _ = child.wait();
            return Err(Error::Other("the downloaded browser did not say its version".into()));
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    let mut said = String::new();
    if let Some(mut out) = child.stdout.take() {
        std::io::Read::read_to_string(&mut out, &mut said)?;
    }
    Ok(said)
}

/// Every process's executable, to know which versions are still running.
/// Blocking.
fn running() -> Vec<PathBuf> {
    let Ok(out) = Command::new("/bin/ps").args(["-axww", "-o", "comm="]).output() else { return Vec::new() };
    String::from_utf8_lossy(&out.stdout).lines().map(|l| PathBuf::from(l.trim())).collect()
}

/// Delete the versions older than `kept`, except one a browser still runs
/// from (another profile's, or this app's own until it restarts), and
/// downloads a copy of the app gave up on. A newer version is another
/// copy's, just unpacked. Blocking.
fn clear_old(folder: &Path, kept: &str, running: &[PathBuf]) {
    let Some(kept) = version_parts(kept) else { return };
    let Ok(entries) = std::fs::read_dir(folder) else { return };
    for entry in entries.flatten() {
        let path = entry.path();
        let Some(name) = entry.file_name().to_str().map(str::to_string) else { continue };
        if name.starts_with(".part-") {
            let modified = entry.metadata().and_then(|m| m.modified()).unwrap_or_else(|_| SystemTime::now());
            if modified.elapsed().is_ok_and(|age| age > STALE_PART) {
                let _ = std::fs::remove_dir_all(&path);
            }
            continue;
        }
        let older = version_parts(&name).is_some_and(|v| v < kept);
        if older && !running.iter().any(|exe| exe.starts_with(&path)) {
            let _ = std::fs::remove_dir_all(&path);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn list(version: &str, url: &str) -> Value {
        json!({ "channels": { "Stable": { "version": version, "downloads": { "chrome": [
            { "platform": "linux64", "url": "https://storage.googleapis.com/chrome-for-testing-public/x/linux64/chrome-linux64.zip" },
            { "platform": "mac-arm64", "url": url },
        ] } } } })
    }

    /// A folder with these versions unpacked in it; `false` leaves one
    /// without its executable, as a half-finished copy would be.
    fn unpacked(versions: &[(&str, bool)]) -> PathBuf {
        let folder = std::env::temp_dir().join(format!("vl-install-{}", uuid::Uuid::new_v4()));
        for (version, whole) in versions {
            let exe = folder.join(version).join(APP).join(EXECUTABLE);
            std::fs::create_dir_all(exe.parent().unwrap()).unwrap();
            if *whole {
                std::fs::write(&exe, "").unwrap();
            }
        }
        folder
    }

    #[test]
    fn the_stable_build_for_this_mac_is_taken_from_googles_list() {
        let url =
            "https://storage.googleapis.com/chrome-for-testing-public/158.0.8127.88/mac-arm64/chrome-mac-arm64.zip";
        let build = stable(&list("158.0.8127.88", url), "mac-arm64").unwrap();
        assert_eq!(build, Build { version: "158.0.8127.88".into(), parts: vec![158, 0, 8127, 88], url: url.into() });
    }

    #[test]
    fn a_build_anywhere_but_googles_own_storage_is_not_downloaded() {
        let url = "https://example.com/chrome-for-testing-public/158.0.8127.88/mac-arm64/chrome-mac-arm64.zip";
        assert!(stable(&list("158.0.8127.88", url), "mac-arm64").is_err());
        let near = "https://storage.googleapis.com.example.com/chrome-for-testing-public/x.zip";
        assert!(stable(&list("158.0.8127.88", near), "mac-arm64").is_err());
    }

    #[test]
    fn a_version_that_is_not_only_numbers_never_names_a_folder() {
        let url = "https://storage.googleapis.com/chrome-for-testing-public/x/mac-arm64/chrome-mac-arm64.zip";
        for odd in ["../../Applications", "158..1", "", "158.0.x", "/tmp"] {
            assert!(stable(&list(odd, url), "mac-arm64").is_err(), "{odd:?}");
        }
    }

    #[test]
    fn the_newest_version_is_run_counting_by_numbers_not_letters() {
        let folder = unpacked(&[("158.0.9.1", true), ("158.0.10.1", true), ("99.0.1.1", true)]);
        let (parts, exe) = newest(&folder).unwrap();
        assert_eq!(parts, vec![158, 0, 10, 1]);
        assert_eq!(exe, folder.join("158.0.10.1").join(APP).join(EXECUTABLE));
    }

    #[test]
    fn a_version_without_its_executable_is_not_run() {
        let folder = unpacked(&[("158.0.1.1", true), ("159.0.1.1", false)]);
        assert_eq!(newest(&folder).unwrap().0, vec![158, 0, 1, 1]);
        assert_eq!(newest(&unpacked(&[])), None);
    }

    #[test]
    fn an_older_version_goes_once_no_browser_runs_from_it() {
        let folder = unpacked(&[("157.0.1.1", true), ("156.0.1.1", true), ("158.0.1.1", true)]);
        let running = [folder.join("157.0.1.1").join(APP).join(EXECUTABLE)];
        clear_old(&folder, "158.0.1.1", &running);
        assert!(folder.join("157.0.1.1").exists(), "a browser still runs from it");
        assert!(!folder.join("156.0.1.1").exists());
        assert!(folder.join("158.0.1.1").exists());

        clear_old(&folder, "158.0.1.1", &[]);
        assert!(!folder.join("157.0.1.1").exists());
    }

    #[test]
    fn a_newer_version_another_copy_of_the_app_just_unpacked_is_kept() {
        let folder = unpacked(&[("158.0.1.1", true), ("159.0.1.1", true)]);
        clear_old(&folder, "158.0.1.1", &[]);
        assert!(folder.join("159.0.1.1").exists());
    }

    #[test]
    fn a_download_another_copy_may_still_be_making_is_left_alone() {
        let folder = unpacked(&[("158.0.1.1", true)]);
        std::fs::create_dir_all(folder.join(".part-now")).unwrap();
        clear_old(&folder, "158.0.1.1", &[]);
        assert!(folder.join(".part-now").exists());
    }

    /// A file of `size` bytes at a local address, whose first answer breaks
    /// off halfway, as a connection that resets does. `ranges`: whether it
    /// answers a Range request with the rest, as Google's storage does, or
    /// with the whole file again. The address, and the Range of each request.
    async fn breaking(size: usize, ranges: bool) -> (String, Vec<u8>, Arc<Mutex<Vec<Option<String>>>>) {
        use axum::body::{Body, Bytes};
        use axum::http::{header, HeaderMap, StatusCode};
        use axum::response::Response;
        let data: Vec<u8> = (0..size).map(|i| (i % 251) as u8).collect();
        let asked = Arc::new(Mutex::new(Vec::new()));
        let (file, seen) = (data.clone(), asked.clone());
        let router = axum::Router::new().route(
            "/chrome.zip",
            axum::routing::get(move |headers: HeaderMap| {
                let (data, seen) = (file.clone(), seen.clone());
                async move {
                    let range = headers.get(header::RANGE).and_then(|v| v.to_str().ok()).map(str::to_string);
                    let first = seen.lock().is_empty();
                    seen.lock().push(range.clone());
                    let from = match (&range, ranges) {
                        (Some(r), true) => r.trim_start_matches("bytes=").trim_end_matches('-').parse().unwrap(),
                        _ => 0,
                    };
                    let rest = Bytes::from(data[from..].to_vec());
                    let body = if first {
                        let half = rest.slice(..rest.len() / 2);
                        let parts: Vec<std::io::Result<Bytes>> = vec![Ok(half), Err(std::io::Error::other("reset"))];
                        Body::from_stream(futures_util::stream::iter(parts))
                    } else {
                        Body::from(rest.clone())
                    };
                    let mut res = Response::builder().header(header::CONTENT_LENGTH, rest.len());
                    if from > 0 {
                        res = res
                            .status(StatusCode::PARTIAL_CONTENT)
                            .header(header::CONTENT_RANGE, format!("bytes {from}-{}/{}", data.len() - 1, data.len()));
                    }
                    res.body(body).unwrap()
                }
            }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://127.0.0.1:{}/chrome.zip", listener.local_addr().unwrap().port());
        tokio::spawn(async move { axum::serve(listener, router).await });
        (url, data, asked)
    }

    /// A connection that reset partway lost the whole download: on a line
    /// that reset every few minutes, Chrome never came.
    #[test]
    fn a_download_that_breaks_off_is_taken_up_where_it_stopped() {
        let (app, root, _) = super::super::tests::app_in_task();
        let to = root.join("chrome.zip");
        tauri::async_runtime::block_on(async {
            let (url, data, asked) = breaking(3 << 20, true).await;
            save(app.handle(), &url, &to, true).await.unwrap();
            assert_eq!(std::fs::read(&to).unwrap(), data);
            let asked = asked.lock().clone();
            assert_eq!(asked.len(), 2);
            assert_eq!(asked[0], None);
            let from: usize =
                asked[1].as_deref().unwrap().trim_start_matches("bytes=").trim_end_matches('-').parse().unwrap();
            assert!(from > 0 && from <= data.len() / 2, "asked only for the rest: {from}");
        });
    }

    #[test]
    fn a_server_that_sends_the_whole_file_again_is_written_from_the_start() {
        let (app, root, _) = super::super::tests::app_in_task();
        let to = root.join("chrome.zip");
        tauri::async_runtime::block_on(async {
            let (url, data, asked) = breaking(3 << 20, false).await;
            save(app.handle(), &url, &to, false).await.unwrap();
            assert_eq!(std::fs::read(&to).unwrap(), data);
            assert_eq!(asked.lock().len(), 2);
        });
    }

    /// The whole download, from Google, into a folder of its own: about
    /// 160 MB, so only by hand
    /// (`cargo test -p villain-layer --lib -- --ignored the_current_chrome`).
    #[test]
    #[ignore = "downloads Chrome from Google"]
    fn the_current_chrome_is_downloaded_unpacked_and_said_how_far_it_has_come() {
        use tauri::Listener;
        let (app, _, _) = super::super::tests::app_in_task();
        let seen = Arc::new(Mutex::new(Vec::new()));
        let (handle, told) = (app.handle().clone(), seen.clone());
        app.listen_any("browser:changed", move |_| {
            told.lock().push(handle.state::<AppState>().browser.download());
        });
        let folder = std::env::temp_dir().join(format!("vl-chrome-{}", uuid::Uuid::new_v4()));

        let exe = tauri::async_runtime::block_on(fetch(app.handle().clone(), folder.clone(), true)).unwrap();

        assert_eq!(Some(exe.clone()), newest(&folder).map(|(_, exe)| exe));
        assert!(exe.starts_with(&folder) && exe.is_file());
        let seen = seen.lock();
        let most = seen.iter().filter_map(|(d, _)| *d).map(|d| (d.done, d.total)).max().unwrap();
        assert!(most.0 > 100 << 20 && most.0 <= most.1, "said how far it had come: {most:?}");
        assert_eq!(seen.last(), Some(&(None, None)), "and that it was done");
        // A browser marks what it downloads, and Gatekeeper then asks before
        // it runs; what the app downloads itself is not marked.
        let marked = Command::new("/usr/bin/xattr")
            .args(["-p", "com.apple.quarantine"])
            .arg(exe.ancestors().nth(3).unwrap())
            .output()
            .unwrap();
        assert!(!marked.status.success(), "quarantined");
        let left: Vec<_> = std::fs::read_dir(&folder).unwrap().flatten().map(|e| e.file_name()).collect();
        assert_eq!(left.len(), 1, "only the version's folder is left: {left:?}");
        std::fs::remove_dir_all(&folder).unwrap();
    }
}

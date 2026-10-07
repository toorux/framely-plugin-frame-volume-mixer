//! Focus providers and an identity-checked mute policy, independent of UI visibility.
use crate::{command, snapshot, status, validate, write_change as change, Change, Stream, Target};
use anyhow::{ensure, Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::PathBuf,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex,
    },
    thread,
    time::{Duration, Instant},
};

#[derive(Clone, Default, Debug, PartialEq, Eq, Serialize)]
pub struct Focus {
    pub gamescope: Option<u64>,
    pub desktop: Option<u32>,
    pub desktop_app: Option<u64>,
    pub vr: Option<u32>,
    pub vr_apps: BTreeSet<u32>,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Identity {
    pub key: String,
    pub app_id: Option<u64>,
    pub ancestors: Vec<u32>,
    pub local: bool,
}
fn lineage(pid: u32) -> Vec<u32> {
    let mut ids = Vec::new();
    let mut next = pid;
    for _ in 0..32 {
        if next <= 1 || ids.contains(&next) {
            break;
        }
        if !ids.is_empty()
            && fs::read_to_string(format!("/proc/{next}/comm")).is_ok_and(|s| {
                matches!(s.trim(), "steam" | "systemd" | "gamescope" | "bash" | "sh")
            })
        {
            break;
        }
        ids.push(next);
        let Ok(raw) = fs::read_to_string(format!("/proc/{next}/stat")) else {
            break;
        };
        next = raw
            .rsplit_once(") ")
            .and_then(|(_, s)| s.split_whitespace().nth(1))
            .and_then(|s| s.parse().ok())
            .unwrap_or(0);
    }
    ids
}
fn steam_id(ids: &[u32]) -> Option<u64> {
    for pid in ids {
        if let Ok(raw) = fs::read(format!("/proc/{pid}/environ")) {
            for entry in raw.split(|b| *b == 0) {
                if let Ok(s) = std::str::from_utf8(entry) {
                    if let Some(v) = s
                        .strip_prefix("SteamAppId=")
                        .or_else(|| s.strip_prefix("SteamGameId="))
                    {
                        if let Ok(id) = v.parse::<u64>() {
                            if id > 0 {
                                return Some(if id > u32::MAX as u64 { id >> 32 } else { id });
                            }
                        }
                    }
                }
            }
        }
    }
    None
}
pub fn identity(s: &Stream) -> Option<Identity> {
    if s.system {
        return None;
    }
    if let Some(id) = s
        .host
        .strip_prefix("lepton-steamlaunch-")
        .and_then(|s| s.parse::<u64>().ok())
    {
        return Some(Identity {
            key: format!("steam:{id}"),
            app_id: Some(id),
            ancestors: Vec::new(),
            local: false,
        });
    }
    let hostname = fs::read_to_string("/proc/sys/kernel/hostname").unwrap_or_default();
    if !s.host.is_empty() && s.host != hostname.trim() {
        return None;
    }
    let ancestors = lineage(s.pid?);
    if ancestors.is_empty() || !PathBuf::from(format!("/proc/{}", s.pid?)).exists() {
        return None;
    }
    let app_id = steam_id(&ancestors);
    let exe = fs::read_link(format!("/proc/{}/exe", s.pid?)).ok();
    let key = app_id
        .map(|id| format!("steam:{id}"))
        .or_else(|| exe.map(|p| format!("exe:{}", p.display())))?;
    Some(Identity {
        key,
        app_id,
        ancestors,
        local: true,
    })
}
fn related(ids: &[u32], focused: u32) -> bool {
    if focused == 0 {
        return false;
    }
    // Exclude shared session launchers: siblings must not gain each other's focus.
    ids.contains(&focused) || lineage(focused).contains(ids.first().unwrap_or(&0))
}
impl Focus {
    pub fn matches(&self, id: &Identity) -> Option<bool> {
        if id.local && id.ancestors.iter().any(|pid| self.vr_apps.contains(pid)) {
            return self.vr.map(|pid| related(&id.ancestors, pid));
        }
        if let (Some(app), Some(focused)) = (id.app_id, self.gamescope) {
            return Some(app == focused);
        }
        if let (Some(app), Some(focused)) = (id.app_id, self.desktop_app) {
            return Some(app == focused);
        }
        if id.local {
            return self.desktop.map(|pid| related(&id.ancestors, pid));
        }
        None
    }
}
fn property(raw: &str, name: &str) -> Option<u64> {
    let line = raw
        .lines()
        .find(|line| line.starts_with(&format!("{name}(")))?;
    let value = line
        .split_once('=')
        .or_else(|| line.split_once("): "))?
        .1
        .trim()
        .trim_start_matches("window id # ");
    let value = value.split(',').next()?.trim();
    if let Some(hex) = value.strip_prefix("0x") {
        u64::from_str_radix(hex, 16).ok()
    } else {
        value.parse().ok()
    }
}
fn desktop_app(pid: u32, class: &str) -> Option<u64> {
    if let Some(id) = steam_id(&lineage(pid)) {
        return Some(id);
    }
    let home = PathBuf::from(std::env::var_os("HOME").unwrap_or_default());
    if let Ok(raw) = fs::read(home.join(".local/share/framely/apk-manager/state.json")) {
        if let Ok(v) = serde_json::from_slice::<Value>(&raw) {
            if let Some(records) = v["records"].as_object() {
                for r in records.values() {
                    if r["removed"] == true {
                        continue;
                    }
                    if let Some(package) =
                        r["metadata"]["package"].as_str().filter(|p| !p.is_empty())
                    {
                        if class
                            .split(|c: char| !c.is_alphanumeric() && c != '.' && c != '_')
                            .any(|part| {
                                part == package || part.strip_prefix("waydroid.") == Some(package)
                            })
                        {
                            return r["steamAppId"].as_u64();
                        }
                    }
                }
            }
        }
    }
    if class.to_ascii_lowercase().contains("waydroid")
        || class.to_ascii_lowercase().contains("lepton")
    {
        None
    } else {
        Some(0)
    }
}
fn xfocus() -> Focus {
    let mut result = Focus::default();
    let display = std::env::var("DISPLAY").unwrap_or_else(|_| ":0".into());
    if let Ok(raw) = command(
        "/usr/bin/xprop",
        &[
            "-display".into(),
            display.clone(),
            "-root".into(),
            "GAMESCOPE_FOCUSED_APP".into(),
            "_NET_ACTIVE_WINDOW".into(),
        ],
    ) {
        result.gamescope = property(&raw, "GAMESCOPE_FOCUSED_APP");
        if let Some(window) = property(&raw, "_NET_ACTIVE_WINDOW") {
            if window == 0 {
                result.desktop = Some(0);
                result.desktop_app = Some(0);
            } else if let Ok(raw) = command(
                "/usr/bin/xprop",
                &[
                    "-display".into(),
                    display,
                    "-id".into(),
                    format!("0x{window:x}"),
                    "_NET_WM_PID".into(),
                    "WM_CLASS".into(),
                ],
            ) {
                result.desktop = property(&raw, "_NET_WM_PID").and_then(|n| u32::try_from(n).ok());
                if let Some(pid) = result.desktop {
                    result.desktop_app = desktop_app(pid, &raw);
                }
            }
        }
    }
    result
}
struct KWinSink(Arc<Mutex<Option<(u32, String, Instant)>>>);
#[zbus::interface(name = "org.framely.VolumeMixer.Focus")]
impl KWinSink {
    fn update(&self, pid: &str, class: &str) {
        if let Ok(pid) = pid.parse::<u32>() {
            *self.0.lock().unwrap() =
                Some((pid, class.chars().take(256).collect(), Instant::now()));
        }
    }
}
struct KWin {
    connection: zbus::blocking::Connection,
    name: String,
    path: PathBuf,
    pid: Arc<Mutex<Option<(u32, String, Instant)>>>,
    owner: String,
}
impl KWin {
    fn start() -> Result<Self> {
        let name = format!("framely-volume-mixer-{}", std::process::id());
        let service = format!("org.framely.VolumeMixer.p{}", std::process::id());
        let pid = Arc::new(Mutex::new(None));
        let connection = zbus::blocking::connection::Builder::session()?
            .method_timeout(Duration::from_secs(1))
            .name(service.as_str())?
            .serve_at("/Focus", KWinSink(pid.clone()))?
            .build()?;
        let owner: String = connection
            .call_method(
                Some("org.freedesktop.DBus"),
                "/org/freedesktop/DBus",
                Some("org.freedesktop.DBus"),
                "GetNameOwner",
                &("org.kde.KWin",),
            )?
            .body()
            .deserialize()?;
        let proxy = zbus::blocking::Proxy::new(
            &connection,
            "org.kde.KWin",
            "/Scripting",
            "org.kde.kwin.Scripting",
        )?;
        let dir = PathBuf::from(
            std::env::var_os("XDG_RUNTIME_DIR").context("Missing runtime directory")?,
        )
        .join(&name);
        fs::create_dir_all(&dir)?;
        let path = dir.join("focus.js");
        // Only report focus; no keyboard, pointer or window state is changed.
        fs::write(
            &path,
            format!(
                r#"function report() {{ var w=workspace.activeWindow || workspace.activeClient; callDBus('{service}', '/Focus', 'org.framely.VolumeMixer.Focus', 'Update', String(w ? w.pid : 0), String(w ? (w.desktopFileName || w.resourceClass || '') : '')); }}
var signal=workspace.windowActivated || workspace.clientActivated;
signal.connect(report); var timer=new QTimer(); timer.interval=1000; timer.timeout.connect(report); timer.start(); report();"#
            ),
        )?;
        let id: i32 = proxy.call(
            "loadScript",
            &(path.to_string_lossy().as_ref(), name.as_str()),
        )?;
        ensure!(id >= 0, "Cannot load KWin focus script");
        zbus::blocking::Proxy::new(
            &connection,
            "org.kde.KWin",
            format!("/Scripting/Script{id}"),
            "org.kde.kwin.Script",
        )?
        .call::<_, _, ()>("run", &())?;
        Ok(Self {
            owner,
            connection,
            name,
            path,
            pid,
        })
    }
    fn read(&self) -> Option<(u32, String)> {
        let owner: String = self
            .connection
            .call_method(
                Some("org.freedesktop.DBus"),
                "/org/freedesktop/DBus",
                Some("org.freedesktop.DBus"),
                "GetNameOwner",
                &("org.kde.KWin",),
            )
            .ok()?
            .body()
            .deserialize()
            .ok()?;
        if owner != self.owner {
            return None;
        }
        self.pid
            .lock()
            .ok()?
            .as_ref()
            .filter(|(_, _, at)| at.elapsed() < Duration::from_secs(3))
            .map(|(pid, class, _)| (*pid, class.clone()))
    }
}
impl Drop for KWin {
    fn drop(&mut self) {
        if let Ok(proxy) = zbus::blocking::Proxy::new(
            &self.connection,
            "org.kde.KWin",
            "/Scripting",
            "org.kde.kwin.Scripting",
        ) {
            let _ = proxy.call::<_, _, bool>("unloadScript", &(self.name.as_str(),));
        }
        let _ = fs::remove_file(&self.path);
        if let Some(dir) = self.path.parent() {
            let _ = fs::remove_dir(dir);
        }
    }
}
unsafe extern "C" {
    fn mixer_vr_open(path: *const libc::c_char) -> bool;
    fn mixer_vr_close();
    fn mixer_vr_pid() -> i64;
    fn mixer_vr_applications(pids: *mut u32, capacity: u32) -> u32;
}
fn vr_paths() -> Vec<PathBuf> {
    let home = PathBuf::from(std::env::var_os("HOME").unwrap_or_default());
    let config = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| home.join(".config"));
    let mut paths = Vec::new();
    if let Ok(raw) = fs::read(config.join("openvr/openvrpaths.vrpath")) {
        if let Ok(v) = serde_json::from_slice::<Value>(&raw) {
            if let Some(runtimes) = v["runtime"].as_array() {
                for runtime in runtimes {
                    if let Some(path) = runtime.as_str() {
                        for arch in ["linuxarm64", "linux64"] {
                            paths.push(
                                PathBuf::from(path).join(format!("bin/{arch}/libopenvr_api.so")),
                            );
                        }
                    }
                }
            }
        }
    }
    paths.push(PathBuf::from(
        "/opt/steamvr/bin/linuxarm64/libopenvr_api.so",
    ));
    paths.push(PathBuf::from("libopenvr_api.so"));
    paths
}
pub struct Reader {
    current: Arc<Mutex<Option<(Focus, Instant)>>>,
    stop: Arc<AtomicBool>,
    worker: Option<thread::JoinHandle<()>>,
}
impl Reader {
    pub fn start() -> Self {
        Self::start_notifying(None)
    }
    fn start_notifying(notify: Option<Arc<dyn Fn() + Send + Sync>>) -> Self {
        let current = Arc::new(Mutex::new(None));
        let stop = Arc::new(AtomicBool::new(false));
        let shared = current.clone();
        let done = stop.clone();
        let worker = thread::spawn(move || {
            let mut kwin = None;
            let mut vr = false;
            let mut scene_pids = BTreeSet::new();
            let mut registered = BTreeSet::new();
            let mut vr_refresh = Instant::now() - Duration::from_secs(2);
            let mut retry = Instant::now() - Duration::from_secs(10);
            while !done.load(Ordering::Relaxed) {
                if retry.elapsed() >= Duration::from_secs(5) {
                    retry = Instant::now();
                    if kwin.is_none() {
                        kwin = KWin::start().ok();
                    }
                    if !vr {
                        for path in vr_paths() {
                            if let Ok(path) =
                                std::ffi::CString::new(path.to_string_lossy().as_bytes())
                            {
                                if unsafe { mixer_vr_open(path.as_ptr()) } {
                                    vr = true;
                                    break;
                                }
                            }
                        }
                    }
                }
                let mut f = xfocus();
                if let Some(k) = &kwin {
                    if let Some((pid, class)) = k.read() {
                        f.desktop = Some(pid);
                        f.desktop_app = desktop_app(pid, &class);
                    } else if retry.elapsed() >= Duration::from_secs(3) {
                        kwin = None;
                    }
                }
                if vr {
                    let pid = unsafe { mixer_vr_pid() };
                    f.vr = u32::try_from(pid).ok();
                    if let Some(pid) = f.vr.filter(|pid| *pid > 0) {
                        scene_pids.insert(pid);
                    }
                    scene_pids.retain(|pid| PathBuf::from(format!("/proc/{pid}")).exists());
                    f.vr_apps.extend(scene_pids.iter().copied());
                    if vr_refresh.elapsed() >= Duration::from_secs(1) {
                        vr_refresh = Instant::now();
                        let mut ids = [0u32; 256];
                        let n =
                            unsafe { mixer_vr_applications(ids.as_mut_ptr(), ids.len() as u32) };
                        registered = ids.into_iter().take(n as usize).collect();
                        // Keep expensive runtime/app discovery off the fast foreground path.
                        if !fs::read_dir("/proc")
                            .ok()
                            .into_iter()
                            .flatten()
                            .filter_map(|e| e.ok())
                            .any(|e| {
                                fs::read_to_string(e.path().join("comm"))
                                    .is_ok_and(|s| s.trim() == "vrserver")
                            })
                        {
                            unsafe { mixer_vr_close() };
                            vr = false;
                            f.vr = None;
                            f.vr_apps.clear();
                            registered.clear();
                        }
                    }
                    f.vr_apps.extend(registered.iter().copied());
                }
                let changed = {
                    let mut cached = shared.lock().unwrap();
                    let changed = cached.as_ref().is_none_or(|(old, _)| old != &f);
                    *cached = Some((f, Instant::now()));
                    changed
                };
                if changed {
                    if let Some(notify) = &notify {
                        notify();
                    }
                }
                thread::sleep(Duration::from_millis(75));
            }
            unsafe { mixer_vr_close() };
        });
        Self {
            current,
            stop,
            worker: Some(worker),
        }
    }
    pub fn get(&self) -> Focus {
        self.current
            .lock()
            .ok()
            .and_then(|v| {
                v.as_ref()
                    .filter(|(_, t)| t.elapsed() < Duration::from_secs(2))
                    .map(|(f, _)| f.clone())
            })
            .unwrap_or_default()
    }
}
impl Drop for Reader {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

#[derive(Clone, Serialize, Deserialize)]
struct Owned {
    cookie: u64,
    target: Target,
    muted: bool,
}
#[derive(Clone, Default, Serialize, Deserialize)]
#[serde(default)]
struct Preference {
    volume: Option<f64>,
    muted: Option<bool>,
    streams: BTreeMap<String, Preference>,
}
#[derive(Default, Serialize, Deserialize)]
#[serde(default)]
struct Saved {
    global: bool,
    enabled: BTreeSet<String>,
    owned: BTreeMap<String, Owned>,
    preferences: BTreeMap<String, Preference>,
}
pub struct Controller {
    saved: Saved,
    path: PathBuf,
    reader: Option<Reader>,
    recover: bool,
    seen: BTreeSet<String>,
    notify: Option<Arc<dyn Fn() + Send + Sync>>,
    cached: Option<Value>,
}
impl Controller {
    pub fn new() -> Self {
        Self::notifying(None)
    }
    pub fn notifying(notify: Option<Arc<dyn Fn() + Send + Sync>>) -> Self {
        let home = PathBuf::from(std::env::var_os("HOME").unwrap_or_default());
        let dir = std::env::var_os("XDG_STATE_HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|| home.join(".local/state"));
        let path = dir.join("framely-volume-mixer/focus.json");
        let saved = fs::read(&path)
            .ok()
            .and_then(|b| serde_json::from_slice(&b).ok())
            .unwrap_or_default();
        Self {
            saved,
            path,
            reader: None,
            recover: true,
            seen: BTreeSet::new(),
            notify,
            cached: None,
        }
    }
    fn save(&self) -> Result<()> {
        fs::create_dir_all(self.path.parent().unwrap())?;
        let bytes = serde_json::to_vec(&self.saved)?;
        if fs::read(&self.path).is_ok_and(|old| old == bytes) {
            return Ok(());
        }
        let temp = self.path.with_extension("tmp");
        fs::write(&temp, bytes)?;
        fs::File::open(&temp)?.sync_all()?;
        fs::rename(temp, &self.path)?;
        Ok(())
    }
    fn restore(&mut self, cookie: u64, streams: &[Stream]) {
        for key in self.saved.owned.clone().keys() {
            let old = &self.saved.owned[key];
            if old.cookie != cookie || !streams.iter().any(|s| s.target == old.target && !s.system)
            {
                self.saved.owned.remove(key);
                continue;
            }
            if change(Change {
                cookie,
                targets: vec![old.target.clone()],
                volume: None,
                muted: Some(old.muted),
            })
            .is_ok()
            {
                self.saved.owned.remove(key);
            }
        }
        let _ = self.save();
    }
    pub fn stop(&mut self) {
        let raw = status();
        if let Ok(streams) = serde_json::from_value::<Vec<Stream>>(raw["streams"].clone()) {
            if let Some(cookie) = raw["cookie"].as_u64() {
                self.restore(cookie, &streams);
            }
        }
        self.reader = None;
    }
    pub fn set(&mut self, params: &Value) -> Result<Value> {
        let raw = status();
        let cookie = raw["cookie"].as_u64().context("音频连接已中断，请刷新。")?;
        ensure!(
            params["cookie"].as_u64() == Some(cookie),
            "音频服务已重启，请刷新后重试"
        );
        let target: Target = serde_json::from_value(params["target"].clone())?;
        let streams: Vec<Stream> = serde_json::from_value(raw["streams"].clone())?;
        let stream = streams
            .iter()
            .find(|s| s.target == target && !s.system)
            .context("播放流已退出、改变或属于系统链路，请刷新后重试")?;
        let id = identity(stream).context("无法关联应用焦点")?;
        let enabled = params["enabled"].as_bool().context("无效开关状态")?;
        if enabled {
            self.saved.enabled.insert(id.key);
        } else {
            self.saved.enabled.remove(&id.key);
        }
        self.save()?;
        Ok(self.tick())
    }
    pub fn global(&mut self, params: &Value) -> Result<Value> {
        let enabled = params["enabled"].as_bool().context("无效开关状态")?;
        // Observe current user settings before applying a global override.
        if enabled && !self.saved.global {
            let _ = self.tick();
        }
        self.saved.global = enabled;
        self.save()?;
        Ok(self.tick())
    }
    pub fn manual(&mut self, mut request: Change) -> Result<Value> {
        let current = snapshot()?;
        validate(&request, &current)?;
        let mut updates = BTreeMap::<String, Vec<&Stream>>::new();
        for target in &request.targets {
            if let Some(s) = current.streams.iter().find(|s| s.target == *target) {
                if let Some(id) = identity(s) {
                    updates.entry(id.key).or_default().push(s);
                }
            }
        }
        for (key, selected) in updates {
            let count = current
                .streams
                .iter()
                .filter(|s| identity(s).is_some_and(|id| id.key == key))
                .count();
            let pref = self.saved.preferences.entry(key).or_default();
            if selected.len() == count {
                if request.volume.is_some() {
                    pref.volume = request.volume;
                    for p in pref.streams.values_mut() {
                        p.volume = None;
                    }
                }
                if request.muted.is_some() {
                    pref.muted = request.muted;
                    for p in pref.streams.values_mut() {
                        p.muted = None;
                    }
                }
            } else {
                for s in selected {
                    let p = pref.streams.entry(s.name.clone()).or_default();
                    if request.volume.is_some() {
                        p.volume = request.volume;
                    }
                    if request.muted.is_some() {
                        p.muted = request.muted;
                    }
                }
            }
        }
        let mut changes = Vec::new();
        if let Some(muted) = request.muted {
            for target in &request.targets {
                let key = format!("{}:{}", request.cookie, target.serial);
                if let Some(old) = self.saved.owned.get_mut(&key) {
                    ensure!(
                        old.target == *target,
                        "播放流已退出、改变或属于系统链路，请刷新后重试"
                    );
                    old.muted = muted;
                    changes.push(target.clone());
                }
            }
        }
        self.save()?;
        // Muted policy targets retain physical mute, but record the user's desired state.
        for target in changes {
            change(Change {
                cookie: request.cookie,
                targets: vec![target.clone()],
                volume: None,
                muted: Some(true),
            })?;
            request.targets.retain(|t| t != &target);
        }
        if !request.targets.is_empty() {
            change(request)?;
        }
        Ok(self.tick())
    }
    pub fn tick(&mut self) -> Value {
        let raw = self.apply(status(), true);
        self.cached = raw["connected"]
            .as_bool()
            .filter(|v| *v)
            .map(|_| raw.clone());
        raw
    }
    pub fn focus_changed(&mut self) -> Value {
        match self.cached.clone() {
            Some(raw) => self.apply(raw, false),
            None => self.tick(),
        }
    }
    fn apply(&mut self, mut raw: Value, refresh: bool) -> Value {
        raw["backgroundMute"] = json!(self.saved.global);
        let Some(cookie) = raw["cookie"].as_u64() else {
            return raw;
        };
        let Ok(streams) = serde_json::from_value::<Vec<Stream>>(raw["streams"].clone()) else {
            return raw;
        };
        if self.recover {
            self.restore(cookie, &streams);
            self.recover = false;
            return self.tick();
        }
        if (self.saved.global || !self.saved.enabled.is_empty()) && self.reader.is_none() {
            self.reader = Some(Reader::start_notifying(self.notify.clone()));
        }
        let focus = self.reader.as_ref().map(|r| r.get()).unwrap_or_default();
        let alive: BTreeSet<_> = streams
            .iter()
            .map(|s| format!("{cookie}:{}", s.target.serial))
            .collect();
        self.saved.owned.retain(|key, _| alive.contains(key));
        self.seen.retain(|key| alive.contains(key));
        for (i, original) in streams.iter().enumerate() {
            let mut s = original.clone();
            if s.system {
                continue;
            }
            let id = identity(&s);
            let key = format!("{cookie}:{}", s.target.serial);
            let mut preference_error = None;
            if refresh && !self.seen.contains(&key) {
                if let Some(pref) = id
                    .as_ref()
                    .and_then(|id| self.saved.preferences.get(&id.key))
                    .cloned()
                {
                    let individual = pref.streams.get(&s.name);
                    let volume = individual.and_then(|p| p.volume).or(pref.volume);
                    let muted = individual.and_then(|p| p.muted).or(pref.muted);
                    for (v, m) in [(volume, None), (None, muted)] {
                        if v.is_none() && m.is_none() {
                            continue;
                        }
                        match change(Change {
                            cookie,
                            targets: vec![s.target.clone()],
                            volume: v,
                            muted: m,
                        }) {
                            Ok(_) => {
                                if v.is_some() {
                                    s.volume = v;
                                    raw["streams"][i]["volume"] = json!(v);
                                }
                                if m.is_some() {
                                    s.muted = m;
                                    raw["streams"][i]["muted"] = json!(m);
                                }
                            }
                            Err(e) => preference_error = Some(e.to_string()),
                        }
                    }
                }
                if preference_error.is_none() {
                    self.seen.insert(key.clone());
                }
            }
            let enabled = id
                .as_ref()
                .is_some_and(|id| self.saved.enabled.contains(&id.key));
            let focused = id.as_ref().and_then(|id| focus.matches(id));
            let key = format!("{cookie}:{}", s.target.serial);
            let background = (enabled || self.saved.global) && focused == Some(false);
            let mut error = preference_error;
            if background {
                if let Some(muted) = s.muted {
                    if !self.saved.owned.contains_key(&key) {
                        self.saved.owned.insert(
                            key.clone(),
                            Owned {
                                cookie,
                                target: s.target.clone(),
                                muted,
                            },
                        );
                        if let Err(e) = self.save() {
                            self.saved.owned.remove(&key);
                            error = Some(e.to_string());
                        }
                    }
                    if error.is_none() && !muted {
                        match change(Change {
                            cookie,
                            targets: vec![s.target.clone()],
                            volume: None,
                            muted: Some(true),
                        }) {
                            Ok(_) => raw["streams"][i]["muted"] = json!(true),
                            Err(e) => error = Some(e.to_string()),
                        }
                    }
                }
            } else if let Some(old) = self.saved.owned.get(&key).cloned() {
                match change(Change {
                    cookie,
                    targets: vec![s.target.clone()],
                    volume: None,
                    muted: Some(old.muted),
                }) {
                    Ok(_) => {
                        raw["streams"][i]["muted"] = json!(old.muted);
                        self.saved.owned.remove(&key);
                        s.muted = Some(old.muted);
                    }
                    Err(e) => error = Some(e.to_string()),
                }
            }
            if let Some(id) = &id {
                if refresh && error.is_none() {
                    let desired = self.saved.owned.get(&key).map(|o| o.muted).or(s.muted);
                    let count = streams
                        .iter()
                        .filter(|stream| identity(stream).is_some_and(|other| other.key == id.key))
                        .count();
                    let pref = self.saved.preferences.entry(id.key.clone()).or_default();
                    if count == 1 {
                        if s.volume.is_some() {
                            if pref.volume != s.volume {
                                for p in pref.streams.values_mut() {
                                    p.volume = None;
                                }
                            }
                            pref.volume = s.volume;
                        }
                        if desired.is_some() {
                            if pref.muted != desired {
                                for p in pref.streams.values_mut() {
                                    p.muted = None;
                                }
                            }
                            pref.muted = desired;
                        }
                    } else {
                        let stream = pref.streams.entry(s.name.clone()).or_default();
                        if s.volume.is_some() {
                            stream.volume = s.volume;
                        }
                        if desired.is_some() {
                            stream.muted = desired;
                        }
                    }
                }
            }
            raw["streams"][i]["backgroundMute"] = json!(enabled);
            raw["streams"][i]["focusSupported"] = json!(id.is_some());
            raw["streams"][i]["focused"] = json!(focused);
            raw["streams"][i]["autoMuted"] = json!(
                background
                    && self.saved.owned.contains_key(&key)
                    && raw["streams"][i]["muted"] == true
            );
            raw["streams"][i]["userMuted"] =
                json!(self.saved.owned.get(&key).map(|o| o.muted).or(s.muted));
            raw["streams"][i]["focusError"] = json!(error);
        }
        raw["backgroundMute"] = json!(self.saved.global);
        if let Err(e) = self.save() {
            raw["focusError"] = json!(e.to_string());
        }
        self.cached = Some(raw.clone());
        raw
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn android(app: u64) -> Identity {
        Identity {
            key: format!("steam:{app}"),
            app_id: Some(app),
            ancestors: vec![],
            local: false,
        }
    }
    #[test]
    fn selects_gamescope_vr_and_desktop_without_treating_unknown_as_background() {
        let mut focus = Focus::default();
        assert_eq!(focus.matches(&android(42)), None);
        focus.gamescope = Some(42);
        assert_eq!(focus.matches(&android(42)), Some(true));
        assert_eq!(focus.matches(&android(43)), Some(false));
        focus.gamescope = None;
        focus.desktop_app = Some(42);
        assert_eq!(focus.matches(&android(42)), Some(true));
        assert_eq!(focus.matches(&android(43)), Some(false));
        focus.desktop_app = None;
        let desktop = Identity {
            key: "desktop".into(),
            app_id: None,
            ancestors: vec![999991, 999992],
            local: true,
        };
        assert_eq!(focus.matches(&desktop), None);
        focus.desktop = Some(999992);
        assert_eq!(focus.matches(&desktop), Some(true));
        focus.desktop = Some(999993);
        assert_eq!(focus.matches(&desktop), Some(false));
        focus.vr_apps.insert(999991);
        focus.vr = Some(999991);
        assert_eq!(focus.matches(&desktop), Some(true));
        focus.vr = Some(999993);
        assert_eq!(focus.matches(&desktop), Some(false));
        focus.vr = None;
        assert_eq!(focus.matches(&desktop), None);
    }
    #[test]
    fn parses_real_xprop_and_no_active_window() {
        assert_eq!(
            property(
                "GAMESCOPE_FOCUSED_APP(CARDINAL) = 3295363555",
                "GAMESCOPE_FOCUSED_APP"
            ),
            Some(3295363555)
        );
        assert_eq!(
            property(
                "_NET_ACTIVE_WINDOW(WINDOW): window id # 0x24",
                "_NET_ACTIVE_WINDOW"
            ),
            Some(36)
        );
        // xprop uses a colon for WINDOW values, unlike CARDINAL.
        assert_eq!(
            property("_NET_ACTIVE_WINDOW(WINDOW) = 0x0", "_NET_ACTIVE_WINDOW"),
            Some(0)
        );
        assert_eq!(
            property("GAMESCOPE_FOCUSED_APP: not found.", "GAMESCOPE_FOCUSED_APP"),
            None
        );
    }
    #[test]
    fn preferences_and_restore_journal_survive_restart_without_audio_node_ids_as_app_keys() {
        let mut saved = Saved::default();
        saved.global = true;
        saved.enabled.insert("steam:42".into());
        saved.preferences.insert(
            "steam:42".into(),
            Preference {
                volume: Some(37.0),
                muted: Some(true),
                ..Default::default()
            },
        );
        saved.owned.insert(
            "7:91".into(),
            Owned {
                cookie: 7,
                target: Target { id: 8, serial: 91 },
                muted: false,
            },
        );
        let decoded: Saved = serde_json::from_slice(&serde_json::to_vec(&saved).unwrap()).unwrap();
        assert!(decoded.global && decoded.enabled.contains("steam:42"));
        assert_eq!(decoded.preferences["steam:42"].volume, Some(37.0));
        assert_eq!(decoded.preferences["steam:42"].muted, Some(true));
        assert!(!decoded.owned["7:91"].muted);
        assert!(!decoded.preferences.contains_key("7:91"));
        let old: Saved = serde_json::from_str(r#"{"enabled":[],"owned":{}}"#).unwrap();
        assert!(!old.global);
    }
}

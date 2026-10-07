pub mod focus;
use anyhow::{ensure, Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::{
    collections::BTreeMap,
    io::Read,
    process::{Command, Stdio},
    thread,
    time::{Duration, Instant},
};

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct Target {
    pub id: u32,
    pub serial: u64,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Stream {
    #[serde(flatten)]
    pub target: Target,
    pub app: String,
    pub binary: String,
    pub pid: Option<u32>,
    pub host: String,
    pub icon_key: Option<String>,
    pub name: String,
    pub state: String,
    pub system: bool,
    pub virtual_device: bool,
    pub volume: Option<f64>,
    pub muted: Option<bool>,
    pub error: Option<String>,
}
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Snapshot {
    pub cookie: u64,
    pub streams: Vec<Stream>,
    pub master: Option<Stream>,
    pub outputs: Vec<Stream>,
}
fn number(v: &Value) -> Option<u64> {
    v.as_u64().or_else(|| v.as_str()?.parse().ok())
}
fn text(v: &Value) -> String {
    v.as_str().unwrap_or("").chars().take(256).collect()
}
fn truth(v: &Value) -> bool {
    v.as_bool().unwrap_or_else(|| v.as_str() == Some("true"))
}
pub fn discover(objects: &Value) -> Result<Snapshot> {
    let objects = objects.as_array().context("pw-dump 未返回对象列表")?;
    let cookie = objects
        .iter()
        .find(|o| o["type"] == "PipeWire:Interface:Core")
        .and_then(|o| number(&o["info"]["cookie"]))
        .context("缺少 PipeWire 服务标识")?;
    let clients: BTreeMap<_, _> = objects
        .iter()
        .filter(|o| o["type"] == "PipeWire:Interface:Client")
        .filter_map(|o| Some((number(&o["id"])?, &o["info"]["props"])))
        .collect();
    let mut streams = Vec::new();
    for o in objects.iter().filter(|o| {
        o["type"] == "PipeWire:Interface:Node"
            && o["info"]["props"]["media.class"] == "Stream/Output/Audio"
    }) {
        let p = &o["info"]["props"];
        let c = number(&p["client.id"]).and_then(|id| clients.get(&id).copied());
        let prop = |key: &str| {
            if p[key].is_null() {
                c.map(|c| &c[key]).unwrap_or(&Value::Null)
            } else {
                &p[key]
            }
        };
        let binary = text(prop("application.process.binary"));
        let app = text(prop("application.name"));
        let name = text(&p["media.name"]);
        let node_name = text(&p["node.name"]);
        let system = truth(&p["node.virtual"])
            || !p["node.link-group"].is_null()
            || matches!(binary.as_str(), "pipewire" | "wireplumber")
            || node_name.starts_with("alsa_loopback_stream.");
        let Some(id) = number(&o["id"]).and_then(|x| u32::try_from(x).ok()) else {
            continue;
        };
        let Some(serial) = number(&p["object.serial"]) else {
            continue;
        };
        streams.push(Stream {
            target: Target { id, serial },
            app: if app.is_empty() {
                if binary.is_empty() {
                    node_name.clone()
                } else {
                    binary.clone()
                }
            } else {
                app
            },
            binary,
            pid: number(prop("application.process.id"))
                .and_then(|x| u32::try_from(x).ok())
                .filter(|x| *x > 0),
            host: text(prop("application.process.host")),
            icon_key: Some(text(prop("application.icon-name"))).filter(|s| !s.is_empty()),
            name: if name.is_empty() { node_name } else { name },
            state: text(&o["info"]["state"]),
            system,
            virtual_device: truth(&p["node.virtual"]),
            volume: None,
            muted: None,
            error: None,
        });
    }
    streams.sort_by(|a, b| {
        (a.system, &a.app, a.pid, a.target.serial).cmp(&(b.system, &b.app, b.pid, b.target.serial))
    });
    let default_name = objects
        .iter()
        .filter(|o| {
            o["type"] == "PipeWire:Interface:Metadata" && o["props"]["metadata.name"] == "default"
        })
        .filter_map(|o| o["metadata"].as_array())
        .flatten()
        .find(|m| number(&m["subject"]) == Some(0) && m["key"] == "default.audio.sink")
        .and_then(|m| {
            let v = if let Some(raw) = m["value"].as_str() {
                serde_json::from_str::<Value>(raw).ok()?
            } else {
                m["value"].clone()
            };
            v["name"].as_str().map(str::to_owned)
        });
    let outputs: Vec<Stream> = objects
        .iter()
        .filter(|o| {
            o["type"] == "PipeWire:Interface:Node"
                && o["info"]["props"]["media.class"] == "Audio/Sink"
        })
        .filter_map(|o| {
            let p = &o["info"]["props"];
            Some(Stream {
                target: Target {
                    id: u32::try_from(number(&o["id"])?).ok()?,
                    serial: number(&p["object.serial"])?,
                },
                app: "系统总音量".into(),
                binary: String::new(),
                pid: None,
                host: String::new(),
                icon_key: None,
                name: if p["node.description"].is_null() {
                    text(&p["node.name"])
                } else {
                    text(&p["node.description"])
                },
                state: text(&o["info"]["state"]),
                system: true,
                virtual_device: truth(&p["node.virtual"]),
                volume: None,
                muted: None,
                error: None,
            })
        })
        .collect();
    let master = default_name
        .as_deref()
        .and_then(|name| {
            objects.iter().find(|o| {
                o["type"] == "PipeWire:Interface:Node" && o["info"]["props"]["node.name"] == name
            })
        })
        .and_then(|o| number(&o["id"]))
        .and_then(|id| outputs.iter().find(|s| u64::from(s.target.id) == id))
        .cloned();
    Ok(Snapshot {
        cookie,
        streams,
        master,
        outputs,
    })
}
// Drain both pipes while waiting: hung audio tools cannot hold the RPC loop forever.
pub fn command(program: &str, args: &[String]) -> Result<String> {
    // Isolated controller integration tests. Release builds always use system tools.
    #[cfg(debug_assertions)]
    let test_program = std::env::var_os("FRAMELY_MIXER_TEST_BIN").map(|dir| {
        std::path::PathBuf::from(dir).join(std::path::Path::new(program).file_name().unwrap())
    });
    #[cfg(debug_assertions)]
    let program = test_program
        .as_ref()
        .and_then(|p| p.to_str())
        .unwrap_or(program);
    let mut child = Command::new(program)
        .args(args)
        .env("LC_ALL", "C")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .with_context(|| format!("启动 {program}（请确认已安装 PipeWire/WirePlumber）"))?;
    let drain = |mut r: Box<dyn Read + Send>| {
        thread::spawn(move || {
            let mut result = Vec::new();
            let mut buf = [0; 8192];
            let mut overflow = false;
            while let Ok(n) = r.read(&mut buf) {
                if n == 0 {
                    break;
                }
                if result.len() + n <= 8 * 1024 * 1024 {
                    result.extend_from_slice(&buf[..n]);
                } else {
                    overflow = true;
                }
            }
            (result, overflow)
        })
    };
    let out = drain(Box::new(child.stdout.take().unwrap()));
    let err = drain(Box::new(child.stderr.take().unwrap()));
    let deadline = Instant::now() + Duration::from_secs(3);
    let status = loop {
        if let Some(status) = child.try_wait()? {
            break Some(status);
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            break None;
        }
        thread::sleep(Duration::from_millis(10));
    };
    let (out, overflow) = out.join().map_err(|_| anyhow::anyhow!("读取输出失败"))?;
    let (err, _) = err.join().map_err(|_| anyhow::anyhow!("读取错误失败"))?;
    let status = status.context(format!("{program} 超时（3 秒）"))?;
    ensure!(
        status.success(),
        "{program}: {}",
        String::from_utf8_lossy(&err).trim()
    );
    ensure!(!overflow, "音频服务返回的数据过大");
    Ok(String::from_utf8(out)?)
}
fn wp(args: &[String]) -> Result<String> {
    command("/usr/bin/wpctl", args)
}
pub fn snapshot() -> Result<Snapshot> {
    let raw: Value = serde_json::from_str(&command("/usr/bin/pw-dump", &[])?)?;
    let mut snapshot = discover(&raw)?;
    if let Some(home) = std::env::var_os("HOME") {
        let path =
            std::path::PathBuf::from(home).join(".local/share/framely/apk-manager/state.json");
        if let Ok(bytes) = std::fs::read(path) {
            if let Ok(registry) = serde_json::from_slice::<Value>(&bytes) {
                resolve_android_names(&mut snapshot, &registry);
            }
        }
    }
    Ok(snapshot)
}
pub fn resolve_android_names(snapshot: &mut Snapshot, registry: &Value) {
    let Some(records) = registry["records"].as_object() else {
        return;
    };
    let names: BTreeMap<_, _> = records
        .values()
        .filter(|r| !truth(&r["removed"]))
        .filter_map(|r| Some((number(&r["steamAppId"])?, text(&r["metadata"]["name"]))))
        .filter(|(_, name)| !name.is_empty())
        .collect();
    for stream in &mut snapshot.streams {
        if stream.system || !stream.app.eq_ignore_ascii_case("waydroid") {
            continue;
        }
        if let Some(id) = stream
            .host
            .strip_prefix("lepton-steamlaunch-")
            .and_then(|id| id.parse::<u64>().ok())
        {
            if let Some(name) = names.get(&id) {
                stream.app = name.clone();
                stream.icon_key = Some(format!("lepton:{id}"));
            }
        }
    }
}
pub fn icon(key: &str) -> Result<Value> {
    use base64::{engine::general_purpose::STANDARD, Engine};
    let home = std::path::PathBuf::from(std::env::var_os("HOME").context("缺少用户目录")?);
    if let Some(id) = key
        .strip_prefix("lepton:")
        .and_then(|s| s.parse::<u64>().ok())
    {
        let registry: Value = serde_json::from_slice(&std::fs::read(
            home.join(".local/share/framely/apk-manager/state.json"),
        )?)?;
        let source = registry["records"]
            .as_object()
            .into_iter()
            .flat_map(|r| r.values())
            .find(|r| number(&r["steamAppId"]) == Some(id) && !truth(&r["removed"]))
            .and_then(|r| r["metadata"]["icon"].as_str())
            .filter(|s| s.starts_with("data:image/png;base64,") && s.len() <= 1024 * 1024);
        return Ok(json!({"src":source}));
    }
    ensure!(
        !key.is_empty()
            && key.len() <= 256
            && key
                .bytes()
                .all(|c| c.is_ascii_alphanumeric() || b"-_.".contains(&c))
            && key != "."
            && key != "..",
        "无效图标名称"
    );
    let roots = [
        home.join(".local/share/icons"),
        home.join(".icons"),
        "/usr/share/icons".into(),
    ];
    let mut paths = Vec::new();
    for root in roots {
        for theme in ["hicolor", "Adwaita", "breeze"] {
            for size in ["48x48", "64x64", "32x32", "128x128", "256x256"] {
                paths.push(
                    root.join(theme)
                        .join(size)
                        .join("apps")
                        .join(format!("{key}.png")),
                );
            }
        }
    }
    paths.push(home.join(".local/share/pixmaps").join(format!("{key}.png")));
    paths.push(std::path::PathBuf::from("/usr/share/pixmaps").join(format!("{key}.png")));
    for path in paths {
        if let Ok(file) = std::fs::File::open(path) {
            let mut bytes = Vec::new();
            file.take(1024 * 1024 + 1).read_to_end(&mut bytes)?;
            if bytes.len() <= 1024 * 1024 && bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
                return Ok(
                    json!({"src":format!("data:image/png;base64,{}",STANDARD.encode(bytes))}),
                );
            }
        }
    }
    Ok(json!({"src":null}))
}
pub fn parse_volume(raw: &str) -> Result<(f64, bool)> {
    let parts: Vec<_> = raw.split_whitespace().collect();
    ensure!(parts.first() == Some(&"Volume:"), "无法读取音量");
    let v: f64 = parts.get(1).context("缺少音量值")?.parse()?;
    ensure!(v.is_finite() && v >= 0.0, "音量值无效");
    Ok((v * 100.0, parts.contains(&"[MUTED]")))
}
pub fn status() -> Value {
    match snapshot() {
        Ok(mut s) => {
            thread::scope(|scope| {
                let mut jobs = Vec::new();
                for stream in s
                    .streams
                    .iter_mut()
                    .filter(|s| !s.system)
                    .chain(s.outputs.iter_mut())
                {
                    jobs.push(scope.spawn(move || {
                        match wp(&["get-volume".into(), stream.target.id.to_string()])
                            .and_then(|x| parse_volume(&x))
                        {
                            Ok((v, m)) => {
                                stream.volume = Some(v);
                                stream.muted = Some(m);
                            }
                            Err(e) => stream.error = Some(format!("{e:#}")),
                        }
                    }));
                }
                for job in jobs {
                    let _ = job.join();
                }
            });
            if let Some(master) = &mut s.master {
                if let Some(output) = s.outputs.iter().find(|o| o.target == master.target) {
                    *master = output.clone();
                }
            }
            json!({"connected":true,"cookie":s.cookie,"streams":s.streams,"master":s.master,"outputs":s.outputs,"error":null})
        }
        Err(e) => {
            json!({"connected":false,"cookie":null,"streams":[],"master":null,"outputs":[],"error":format!("{e:#}")})
        }
    }
}
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Change {
    pub cookie: u64,
    pub targets: Vec<Target>,
    pub volume: Option<f64>,
    pub muted: Option<bool>,
}
pub fn validate(change: &Change, current: &Snapshot) -> Result<()> {
    validate_mode(change, current, Control::Streams)
}
#[derive(Clone, Copy)]
enum Control {
    Streams,
    Master,
    Outputs,
}
fn validate_mode(change: &Change, current: &Snapshot, mode: Control) -> Result<()> {
    ensure!(
        change.cookie == current.cookie,
        "音频服务已重启，请刷新后重试"
    );
    ensure!(
        !change.targets.is_empty() && change.targets.len() <= 64,
        "请选择 1–64 个播放流"
    );
    ensure!(
        change.volume.is_some() ^ change.muted.is_some(),
        "一次只能设置音量或静音"
    );
    if let Some(v) = change.volume {
        ensure!(
            v.is_finite() && (0.0..=100.0).contains(&v),
            "音量必须在 0–100% 之间"
        );
    }
    if matches!(mode, Control::Outputs) {
        ensure!(
            change.targets.len() == 1
                && current
                    .outputs
                    .iter()
                    .any(|s| s.target == change.targets[0]),
            "输出设备已退出或改变，请刷新后重试"
        );
        return Ok(());
    }
    if matches!(mode, Control::Master) {
        ensure!(
            change.targets.len() == 1
                && current
                    .master
                    .as_ref()
                    .is_some_and(|s| s.target == change.targets[0]),
            "默认播放设备已切换或不可用，请刷新后重试"
        );
        return Ok(());
    }
    let mut ids = std::collections::BTreeSet::new();
    for target in &change.targets {
        ensure!(ids.insert(target.serial), "重复的播放流");
        ensure!(
            current
                .streams
                .iter()
                .any(|s| s.target == *target && !s.system),
            "播放流已退出、改变或属于系统链路，请刷新后重试"
        );
    }
    Ok(())
}
pub fn change(change: Change) -> Result<Value> {
    change_mode(change, Control::Streams)
}
pub fn master_change(change: Change) -> Result<Value> {
    change_mode(change, Control::Master)
}
pub fn output_change(change: Change) -> Result<Value> {
    change_mode(change, Control::Outputs)
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DefaultOutput {
    pub cookie: u64,
    pub target: Target,
}
pub fn set_default(change: DefaultOutput) -> Result<Value> {
    let current = snapshot()?;
    ensure!(
        change.cookie == current.cookie,
        "音频服务已重启，请刷新后重试"
    );
    ensure!(
        current.outputs.iter().any(|s| s.target == change.target),
        "输出设备已退出或改变，请刷新后重试"
    );
    wp(&["set-default".into(), change.target.id.to_string()])?;
    Ok(status())
}
fn change_mode(change: Change, mode: Control) -> Result<Value> {
    write_mode(change, mode)?;
    Ok(status())
}
/// Write without a redundant full volume readback; every target remains identity checked.
pub fn write_change(change: Change) -> Result<()> {
    write_mode(change, Control::Streams)
}
fn write_mode(change: Change, mode: Control) -> Result<()> {
    let initial = snapshot()?;
    validate_mode(&change, &initial, mode)?;
    let mut applied = 0;
    for target in &change.targets {
        // Re-enumerate each target immediately before writing; never use PID-wide
        // writes that could also affect capture streams or virtual system nodes.
        let current = if applied == 0 {
            initial.clone()
        } else {
            snapshot()?
        };
        validate_mode(
            &Change {
                cookie: change.cookie,
                targets: vec![target.clone()],
                volume: change.volume,
                muted: change.muted,
            },
            &current,
            mode,
        )?;
        let args = if let Some(v) = change.volume {
            vec![
                "set-volume".into(),
                target.id.to_string(),
                format!("{v:.2}%"),
                "--limit".into(),
                "1.0".into(),
            ]
        } else {
            vec![
                "set-mute".into(),
                target.id.to_string(),
                if change.muted == Some(true) { "1" } else { "0" }.into(),
            ]
        };
        wp(&args).with_context(|| format!("设置失败；已更新 {applied} 个流，请刷新确认"))?;
        applied += 1;
    }
    Ok(())
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn resolves_lepton_names_without_using_container_pids() {
        let mut raw = fixture();
        raw[1]["info"]["props"]["application.name"] = json!("Waydroid");
        raw[1]["info"]["props"]["application.process.host"] =
            json!("lepton-steamlaunch-3295363555");
        let mut state = discover(&raw).unwrap();
        let registry = json!({"records":{"bilibili":{"steamAppId":3295363555u64,"metadata":{"name":"哔哩哔哩HD"}},"removed":{"steamAppId":2518847852u64,"removed":true,"metadata":{"name":"已移除"}}}});
        resolve_android_names(&mut state, &registry);
        assert_eq!(state.streams[0].app, "哔哩哔哩HD");
        assert_eq!(
            state.streams[0].icon_key.as_deref(),
            Some("lepton:3295363555")
        );
        assert_eq!(state.streams[1].app, "Waydroid"); // System nodes remain untouched.
        state.streams[0].app = "Waydroid".into();
        state.streams[0].host = "lepton-steamlaunch-2518847852".into();
        resolve_android_names(&mut state, &registry);
        assert_eq!(state.streams[0].app, "Waydroid");
        state.streams[0].host = "unknown-container".into();
        resolve_android_names(&mut state, &registry);
        assert_eq!(state.streams[0].app, "Waydroid");
    }
    #[test]
    fn default_sink_is_separate_and_rejects_device_changes() {
        let mut raw = fixture();
        let objects = raw.as_array_mut().unwrap();
        objects.push(json!({"type":"PipeWire:Interface:Metadata","props":{"metadata.name":"default"},"metadata":[{"subject":0,"key":"default.audio.sink","value":{"name":"speaker"}}]}));
        objects.push(json!({"id":73,"type":"PipeWire:Interface:Node","info":{"props":{"media.class":"Audio/Sink","node.name":"speaker","node.description":"Speaker","object.serial":173}}}));
        let mut state = discover(&raw).unwrap();
        let c = Change {
            cookie: 7,
            targets: vec![Target {
                id: 73,
                serial: 173,
            }],
            volume: Some(40.0),
            muted: None,
        };
        assert!(validate_mode(&c, &state, Control::Master).is_ok());
        assert_eq!(state.outputs.len(), 1);
        assert!(validate_mode(&c, &state, Control::Outputs).is_ok());
        assert!(validate(&c, &state).is_err());
        state.master.as_mut().unwrap().target.serial = 174;
        assert!(validate_mode(&c, &state, Control::Master).is_err());
        state.master = None;
        assert!(validate_mode(&c, &state, Control::Master).is_err());
        assert!(validate_mode(&c, &state, Control::Outputs).is_ok());
        state.outputs[0].target.serial += 1;
        assert!(validate_mode(&c, &state, Control::Outputs).is_err());
    }
    fn fixture() -> Value {
        json!([
            {"type":"PipeWire:Interface:Core","info":{"cookie":7}},
            {"id":10,"type":"PipeWire:Interface:Client","info":{"props":{"application.name":"Game","application.process.binary":"game","application.process.id":"4321"}}},
            {"id":20,"type":"PipeWire:Interface:Node","info":{"state":"running","props":{"media.class":"Stream/Output/Audio","client.id":10,"object.serial":100,"media.name":"music"}}},
            {"id":21,"type":"PipeWire:Interface:Node","info":{"props":{"media.class":"Stream/Output/Audio","client.id":10,"object.serial":"101","node.virtual":true}}},
            {"id":22,"type":"PipeWire:Interface:Node","info":{"props":{"media.class":"Stream/Input/Audio","object.serial":102}}}
        ])
    }
    #[test]
    fn discovers_native_and_pulse_streams_but_excludes_capture() {
        let s = discover(&fixture()).unwrap();
        assert_eq!(s.streams.len(), 2);
        assert_eq!(s.streams[0].pid, Some(4321));
        assert_eq!(s.streams[0].app, "Game");
        assert!(!s.streams[0].system);
        assert!(s.streams[1].system);
    }
    #[test]
    fn rejects_reused_ids_restarts_system_nodes_and_invalid_values() {
        let s = discover(&fixture()).unwrap();
        let mut c = Change {
            cookie: 7,
            targets: vec![Target {
                id: 20,
                serial: 100,
            }],
            volume: Some(50.0),
            muted: None,
        };
        assert!(validate(&c, &s).is_ok());
        c.targets[0].serial = 99;
        assert!(validate(&c, &s).is_err());
        c.targets[0] = Target {
            id: 21,
            serial: 101,
        };
        assert!(validate(&c, &s).is_err());
        c.targets[0] = Target {
            id: 20,
            serial: 100,
        };
        c.cookie = 8;
        assert!(validate(&c, &s).is_err());
        c.cookie = 7;
        c.volume = Some(101.0);
        assert!(validate(&c, &s).is_err());
        c.volume = Some(f64::NAN);
        assert!(validate(&c, &s).is_err());
        c.volume = Some(50.0);
        c.muted = Some(false);
        assert!(validate(&c, &s).is_err());
    }
    #[test]
    fn parses_cubic_ui_volume_without_converting_again() {
        assert_eq!(
            parse_volume("Volume: 0.35 [MUTED]\n").unwrap(),
            (35.0, true)
        );
        assert!(parse_volume("Volume: NaN").is_err());
        assert!(parse_volume("error").is_err());
    }
}

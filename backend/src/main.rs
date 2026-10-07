use anyhow::{ensure, Result};
use framely_volume_mixer::{
    icon, master_change, output_change, set_default, status, Change, DefaultOutput,
};
use serde_json::{json, Value};
use std::{
    io::{BufRead, Read, Write},
    sync::mpsc,
    time::{Duration, Instant},
};
fn output(v: Value) {
    let mut out = std::io::stdout().lock();
    let _ = writeln!(out, "{v}");
    let _ = out.flush();
}
fn main() -> Result<()> {
    ensure!(
        unsafe { libc::geteuid() } != 0,
        "请以 SteamOS 音频会话用户运行，无需 root"
    );
    let uid = unsafe { libc::geteuid() };
    let runtime = format!("/run/user/{uid}");
    std::env::set_var("XDG_RUNTIME_DIR", &runtime);
    std::env::set_var(
        "DBUS_SESSION_BUS_ADDRESS",
        format!("unix:path={runtime}/bus"),
    );
    std::env::set_var("PIPEWIRE_REMOTE", "pipewire-0");
    if std::env::args().any(|a| a == "--status") {
        output(status());
        return Ok(());
    }
    if std::env::args().any(|a| a == "--focus") {
        let reader = framely_volume_mixer::focus::Reader::start();
        std::thread::sleep(Duration::from_secs(2));
        output(serde_json::to_value(reader.get())?);
        return Ok(());
    }
    let (tx, rx) = mpsc::sync_channel(32);
    std::thread::spawn(move || {
        let mut input = std::io::BufReader::new(std::io::stdin());
        loop {
            let mut line = Vec::new();
            match input.by_ref().take(65538).read_until(b'\n', &mut line) {
                Ok(0) | Err(_) => break,
                _ if line.len() > 65537 => {
                    output(json!({"error":"请求过大"}));
                    break;
                }
                _ => {
                    if tx.send(line).is_err() {
                        break;
                    }
                }
            }
        }
    });
    let mut policy = framely_volume_mixer::focus::Controller::new();
    let mut active = false;
    let interval = Duration::from_millis(500);
    let mut next_status = Instant::now() + interval;
    loop {
        match rx.recv_timeout(next_status.saturating_duration_since(Instant::now())) {
            Ok(line) => {
                let req: Value = match serde_json::from_slice(&line) {
                    Ok(v) => v,
                    Err(e) => {
                        output(json!({"error":e.to_string()}));
                        continue;
                    }
                };
                let result = (|| -> Result<Value> {
                    Ok(match req["method"].as_str().unwrap_or("") {
                        "framely.lifecycle.start" => {
                            active = true;
                            json!({"ready":true})
                        }
                        "framely.lifecycle.stop" => {
                            active = false;
                            policy.stop();
                            json!({"stopped":true})
                        }
                        "framely.ui.visibility" => json!({"accepted":true}),
                        "status.get" => policy.tick(),
                        "focus.set" => policy.set(&req["params"])?,
                        "focus.global" => policy.global(&req["params"])?,
                        "icon.get" => icon(req["params"]["key"].as_str().unwrap_or(""))?,
                        "outputs.set" => {
                            output_change(serde_json::from_value::<Change>(
                                req["params"].clone(),
                            )?)?;
                            policy.tick()
                        }
                        "outputs.default" => {
                            set_default(serde_json::from_value::<DefaultOutput>(
                                req["params"].clone(),
                            )?)?;
                            policy.tick()
                        }
                        "master.set" => {
                            master_change(serde_json::from_value::<Change>(
                                req["params"].clone(),
                            )?)?;
                            policy.tick()
                        }
                        "streams.set" => policy
                            .manual(serde_json::from_value::<Change>(req["params"].clone())?)?,
                        _ => anyhow::bail!("未知方法"),
                    })
                })();
                output(match result {
                    Ok(v) => json!({"id":req["id"],"result":v}),
                    Err(e) => json!({"id":req["id"],"error":format!("{e:#}")}),
                });
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {
                if active {
                    output(json!({"event":"status","data":policy.tick()}));
                }
                next_status = Instant::now() + interval;
            }
            Err(mpsc::RecvTimeoutError::Disconnected) => break,
        }
    }
    policy.stop();
    Ok(())
}

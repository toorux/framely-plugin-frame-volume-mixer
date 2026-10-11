//! Best-effort local names for legacy Steam identities. No Steam/network access required.
use serde_json::{json, Map, Value};
use std::{collections::BTreeMap, fs, path::Path};

fn string(input: &mut &[u8]) -> Option<String> {
    let end = input.iter().position(|b| *b == 0)?;
    let value = std::str::from_utf8(&input[..end]).ok()?.to_owned();
    *input = &input[end + 1..];
    Some(value)
}
fn object(input: &mut &[u8], depth: usize, budget: &mut usize) -> Option<Value> {
    if depth > 16 {
        return None;
    }
    let mut result = Map::new();
    loop {
        let kind = *input.first()?;
        *input = &input[1..];
        if kind == 8 {
            return Some(Value::Object(result));
        }
        *budget = budget.checked_sub(1)?;
        let key = string(input)?.to_ascii_lowercase();
        let value = match kind {
            0 => object(input, depth + 1, budget)?,
            1 => json!(string(input)?),
            2 => {
                let bytes: [u8; 4] = input.get(..4)?.try_into().ok()?;
                *input = &input[4..];
                json!(u32::from_le_bytes(bytes))
            }
            3 | 4 | 6 | 7 => {
                let size = if kind == 7 { 8 } else { 4 };
                *input = input.get(size..)?;
                Value::Null
            }
            _ => return None,
        };
        result.insert(key, value);
    }
}
fn bounded(path: &Path) -> Option<Vec<u8>> {
    if fs::metadata(path).ok()?.len() > 4 * 1024 * 1024 {
        return None;
    }
    fs::read(path).ok()
}
pub(crate) fn lookup(home: &Path) -> BTreeMap<u64, String> {
    let mut names = BTreeMap::new();
    for root in [home.join(".local/share/Steam"), home.join(".steam/steam")] {
        if let Ok(entries) = fs::read_dir(root.join("steamapps")) {
            for entry in entries.flatten() {
                let file = entry.file_name().to_string_lossy().into_owned();
                let Some(id) = file
                    .strip_prefix("appmanifest_")
                    .and_then(|s| s.strip_suffix(".acf"))
                    .and_then(|s| s.parse::<u64>().ok())
                else {
                    continue;
                };
                if let Some(bytes) = bounded(&entry.path()) {
                    if let Ok(raw) = std::str::from_utf8(&bytes) {
                        for line in raw.lines() {
                            let mut parts = line.split('"');
                            if parts.next().is_some()
                                && parts.next().is_some_and(|k| k.eq_ignore_ascii_case("name"))
                            {
                                let _ = parts.next();
                                if let Some(name) = parts.next().filter(|n| !n.is_empty()) {
                                    names.insert(id, name.to_owned());
                                }
                            }
                        }
                    }
                }
            }
        }
        if let Ok(users) = fs::read_dir(root.join("userdata")) {
            for user in users.flatten() {
                let Some(bytes) = bounded(&user.path().join("config/shortcuts.vdf")) else {
                    continue;
                };
                let Some(raw) = object(&mut bytes.as_slice(), 0, &mut 10000) else {
                    continue;
                };
                for shortcut in raw["shortcuts"]
                    .as_object()
                    .into_iter()
                    .flat_map(|m| m.values())
                {
                    if let (Some(id), Some(name)) = (
                        shortcut["appid"].as_u64(),
                        shortcut["appname"].as_str().filter(|n| !n.is_empty()),
                    ) {
                        names.insert(id, name.to_owned());
                    }
                }
            }
        }
    }
    names
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn reads_unsigned_shortcut_ids_and_rejects_truncation() {
        let mut bytes = b"\0shortcuts\0\x000\0\x02appid\0".to_vec();
        bytes.extend(2838518023u32.to_le_bytes());
        bytes.extend(b"\x01AppName\0Old muted app\0\x00tags\0\x010\0VR\0\x08\x08\x08\x08");
        let raw = object(&mut bytes.as_slice(), 0, &mut 10000).unwrap();
        assert_eq!(raw["shortcuts"]["0"]["appid"], 2838518023u64);
        assert_eq!(raw["shortcuts"]["0"]["appname"], "Old muted app");
        assert!(object(&mut &bytes[..20], 0, &mut 10000).is_none());
        assert!(object(&mut bytes.as_slice(), 0, &mut 1).is_none());
    }
}

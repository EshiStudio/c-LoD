//! Pre-boot spigot.yml fixup: raise `entity-tracking-range.display` so the
//! vanilla client actually RECEIVES display entities at LOD distances.
//!
//! Runs from `cplugin_init` — before the kernel reads its config — so the
//! change is effective on the very next boot without a second restart.
//!
//! File-only, no JVM: fully unit-tested.

/// Ensure the tracking range for display entities is at least `needed`
/// blocks. Returns a human-readable action ("ok", "raised", "skipped: ...").
pub fn ensure_tracking_range(text: &str, needed: u32) -> (String, Option<String>) {
    let lines: Vec<&str> = text.lines().collect();
    let mut out: Vec<String> = lines.iter().map(|s| s.to_string()).collect();
    // find the "entity-tracking-range:" key at any indent
    let mut tr_idx = None;
    let mut tr_indent = 0usize;
    for (i, l) in out.iter().enumerate() {
        let t = l.trim_start();
        if t.starts_with("entity-tracking-range:") {
            tr_idx = Some(i);
            tr_indent = l.len() - t.len();
            break;
        }
    }
    match tr_idx {
        Some(i) => {
            // find child "display:" within the block (deeper indent)
            let child_indent = tr_indent + 2;
            let mut found = None;
            for (j, l) in out.iter().enumerate().skip(i + 1) {
                let t = l.trim_start();
                if t.is_empty() {
                    continue;
                }
                let ind = l.len() - t.len();
                if ind <= tr_indent {
                    break; // left the block
                }
                if ind == child_indent && t.starts_with("display:") {
                    found = Some(j);
                    break;
                }
            }
            match found {
                Some(j) => {
                    let cur = out[j]
                        .split(':')
                        .nth(1)
                        .and_then(|v| v.trim().parse::<u32>().ok())
                        .unwrap_or(0);
                    if cur >= needed {
                        (format!("ok (display tracking {cur} >= {needed})"), None)
                    } else {
                        out[j] = format!("{}display: {}", " ".repeat(child_indent), needed);
                        (
                            format!("raised display tracking {} -> {} blocks", cur, needed),
                            Some(out.join("\n") + "\n"),
                        )
                    }
                }
                None => {
                    out.insert(
                        i + 1,
                        format!("{}display: {}", " ".repeat(child_indent), needed),
                    );
                    (
                        format!("added display tracking = {needed} blocks"),
                        Some(out.join("\n") + "\n"),
                    )
                }
            }
        }
        None => {
            // find world-settings.default, else append a full block
            let mut ws = None;
            for (i, l) in out.iter().enumerate() {
                if l.trim_start().starts_with("world-settings:") {
                    ws = Some(i);
                    break;
                }
            }
            match ws {
                Some(wi) => {
                    // find "default:" child
                    let mut di = None;
                    let ws_indent = out[wi].len() - out[wi].trim_start().len();
                    for (j, l) in out.iter().enumerate().skip(wi + 1) {
                        let t = l.trim_start();
                        if t.is_empty() {
                            continue;
                        }
                        let ind = l.len() - t.len();
                        if ind <= ws_indent {
                            break;
                        }
                        if t.starts_with("default:") {
                            di = Some((j, ind));
                            break;
                        }
                    }
                    let base = di
                        .map(|(j, ind)| (j + 1, ind + 2))
                        .unwrap_or((wi + 1, ws_indent + 2));
                    out.insert(
                        base.0,
                        format!("{}entity-tracking-range:", " ".repeat(base.1)),
                    );
                    out.insert(
                        base.0 + 1,
                        format!("{}display: {}", " ".repeat(base.1 + 2), needed),
                    );
                    (
                        format!("added entity-tracking-range.display = {needed}"),
                        Some(out.join("\n") + "\n"),
                    )
                }
                None => {
                    out.push(String::new());
                    out.push("world-settings:".into());
                    out.push("  default:".into());
                    out.push("    entity-tracking-range:".into());
                    out.push(format!("      display: {needed}"));
                    (
                        format!("appended world-settings block (display = {needed})"),
                        Some(out.join("\n") + "\n"),
                    )
                }
            }
        }
    }
}

/// Convenience: load file, patch, save. Returns the action description.
pub fn apply_to_file(path: &std::path::Path, needed: u32) -> std::io::Result<String> {
    let text = if path.exists() {
        std::fs::read_to_string(path)?
    } else {
        String::new()
    };
    let (note, new) = ensure_tracking_range(&text, needed);
    if let Some(new) = new {
        // spigot.yml may be absent on a brand-new server dir: create a
        // minimal one — the kernel merges missing keys with defaults.
        std::fs::write(path, new)?;
    }
    Ok(note)
}

#[cfg(test)]
mod tests {
    use super::*;

    const REAL_SHAPE: &str = "\
settings:
  log-named-entities: false
  timeout-time: 60
world-settings:
  default:
    verbose: false
    entity-tracking-range:
      players: 128
      animals: 96
      monsters: 96
      misc: 96
      display: 128
      other: 64
    merge-radius:
      item: 0.5
";

    #[test]
    fn raises_existing_display_value() {
        let (note, new) = ensure_tracking_range(REAL_SHAPE, 1024);
        assert!(note.contains("raised"), "{note}");
        let new = new.unwrap();
        assert!(new.contains("display: 1024"));
        assert!(new.contains("players: 128"), "siblings untouched");
        assert!(new.contains("item: 0.5"), "trailing sections intact");
        // idempotent when already high enough
        let (note2, new2) = ensure_tracking_range(&new, 512);
        assert!(note2.contains("ok"));
        assert!(new2.is_none());
    }

    #[test]
    fn adds_missing_display_key() {
        let yml = "world-settings:\n  default:\n    entity-tracking-range:\n      players: 128\n";
        let (note, new) = ensure_tracking_range(yml, 512);
        assert!(note.contains("added"), "{note}");
        let new = new.unwrap();
        let lines: Vec<&str> = new.lines().collect();
        let idx = lines.iter().position(|l| l.contains("display:")).unwrap();
        assert!(lines[idx].contains("512"));
        assert!(
            idx > lines
                .iter()
                .position(|l| l.contains("entity-tracking-range"))
                .unwrap()
        );
    }

    #[test]
    fn adds_missing_tracking_block() {
        let yml = "world-settings:\n  default:\n    verbose: false\n";
        let (note, new) = ensure_tracking_range(yml, 512);
        assert!(note.contains("added"), "{note}");
        let new = new.unwrap();
        assert!(new.contains("entity-tracking-range:"));
        assert!(new.contains("display: 512"));
        assert!(new.contains("verbose: false"));
    }

    #[test]
    fn appends_when_no_world_settings() {
        let yml = "settings:\n  timeout-time: 60\n";
        let (note, new) = ensure_tracking_range(yml, 512);
        assert!(note.contains("appended"), "{note}");
        let new = new.unwrap();
        assert!(new.contains("world-settings:"));
        assert!(new.contains("display: 512"));
    }

    #[test]
    fn empty_file_gets_full_block() {
        let (note, new) = ensure_tracking_range("", 1024);
        assert!(note.contains("appended"), "{note}");
        let new = new.unwrap();
        assert!(new.contains("display: 1024"));
    }

    #[test]
    fn apply_to_file_creates_minimal() {
        let dir = std::env::temp_dir().join(format!("clod-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("spigot.yml");
        let _ = std::fs::remove_file(&p);
        let note = apply_to_file(&p, 2048).unwrap();
        assert!(note.contains("appended"));
        let content = std::fs::read_to_string(&p).unwrap();
        assert!(content.contains("display: 2048"));
        let _ = std::fs::remove_file(&p);
        let _ = std::fs::remove_dir(&dir);
    }
}

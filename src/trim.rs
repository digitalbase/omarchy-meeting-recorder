//! Make a shortened copy, never modifying the original meeting.
use std::path::{Path, PathBuf};
use std::process::Command;

pub fn copy_until(dir: &Path, until_us: i64) -> Result<PathBuf, String> {
    if until_us <= 0 {
        return Err("Choose a cut point after the start of the recording".into());
    }
    let (_, mut manifest) = crate::meeting::open(dir).ok_or("Cannot open meeting")?;
    let parent = dir.parent().ok_or("Meeting has no parent folder")?;
    let name = dir
        .file_name()
        .ok_or("Meeting has no folder name")?
        .to_string_lossy();
    let mut n = 1;
    let out = loop {
        let candidate = parent.join(format!("{name} (trimmed {n})"));
        match std::fs::create_dir(&candidate) {
            Ok(()) => break candidate,
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => n += 1,
            Err(e) => return Err(e.to_string()),
        }
    };
    let result = (|| {
        let mut count = 0;
        for file in [
            "audio.ogg",
            "mic.ogg",
            "computer.ogg",
            ".tracks/mic.ogg",
            ".tracks/computer.ogg",
            ".tracks/source.ogg",
            "webcam.mp4",
        ] {
            let source = dir.join(file);
            if !source.is_file() {
                continue;
            }
            let target = out.join(file);
            std::fs::create_dir_all(target.parent().unwrap()).map_err(|e| e.to_string())?;
            let mut cmd = Command::new("ffmpeg");
            cmd.args(["-v", "error", "-nostdin", "-i"])
                .arg(&source)
                .args(["-t", &format!("{:.6}", until_us as f64 / 1_000_000.0)]);
            if file.ends_with(".ogg") {
                cmd.args(["-c:a", "libopus"]);
            }
            let output = cmd.arg(&target).output().map_err(|e| e.to_string())?;
            if !output.status.success() {
                return Err(format!(
                    "Could not trim {file}: {}",
                    String::from_utf8_lossy(&output.stderr)
                ));
            }
            count += 1;
        }
        if count == 0 {
            return Err("No recording files found".into());
        }
        let (mic, computer) = crate::export::tracks(&out);
        if !out.join(".tracks/source.ogg").is_file() && !(mic.is_file() && computer.is_file()) {
            let audio = out.join("audio.ogg");
            if !audio.is_file() {
                return Err("The source tracks needed for retranscription are missing".into());
            }
            std::fs::create_dir_all(out.join(".tracks")).map_err(|e| e.to_string())?;
            std::fs::copy(audio, out.join(".tracks/source.ogg")).map_err(|e| e.to_string())?;
            manifest.imported = Some("audio.ogg".into());
        }
        manifest.title = format!("{} (trimmed)", manifest.title);
        manifest.duration_secs = until_us / 1_000_000;
        manifest.chapters.retain(|chapter| chapter.start_ms < until_us / 1000);
        let transcript = dir.join("transcript.md");
        if transcript.is_file() {
            let text = std::fs::read_to_string(transcript).map_err(|e| e.to_string())?;
            let text = trim_transcript(&text, until_us / 1000);
            let text = crate::chapters::apply_to_markdown(&text, &manifest.chapters);
            std::fs::write(out.join("transcript.md"), text).map_err(|e| e.to_string())?;
        }
        let notes = dir.join(crate::notes::NOTES_FILE);
        if notes.is_file() {
            std::fs::copy(notes, out.join(crate::notes::NOTES_FILE)).map_err(|e| e.to_string())?;
        }
        crate::meeting::write(&out, &manifest).map_err(|e| e.to_string())?;
        Ok(out.clone())
    })();
    if result.is_err() {
        // Only this newly created copy is removed on failure.
        let _ = std::fs::remove_dir_all(&out);
    }
    result
}

fn trim_transcript(text: &str, until_ms: i64) -> String {
    let mut kept = String::new();
    let mut keep = true;
    for line in text.lines() {
        if let Some(rest) = line.strip_prefix("**[")
            && let Some((time, _)) = rest.split_once("] ")
        {
            let seconds = time.split(':').try_fold(0i64, |total, part| {
                total.checked_mul(60)?.checked_add(part.parse::<i64>().ok()?)
            });
            keep = seconds.is_none_or(|seconds| seconds.saturating_mul(1000) < until_ms);
        }
        if line.starts_with("- **Duration:** ") {
            kept.push_str(&format!("- **Duration:** {}\n", crate::chapters::clock(until_ms)));
        } else if keep {
            kept.push_str(line);
            kept.push('\n');
        }
    }
    kept
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn trims_entries_at_or_after_cut_and_their_continuations() {
        let text = "- **Duration:** 3:00:00\n\n## Transcript\n\n**[59:58] You:** Keep\ncontinued\n\n**[1:00:00] Remote:** Remove\ncontinued too\n";
        let result = trim_transcript(text, 3_600_000);
        assert!(result.contains("- **Duration:** 1:00:00"));
        assert!(result.contains("Keep\ncontinued"));
        assert!(!result.contains("Remove"));
        assert!(!result.contains("continued too"));
    }

    #[test]
    fn rejects_empty_or_negative_cut_before_creating_a_copy() {
        for cut in [0, -1] {
            assert!(
                copy_until(Path::new("/nonexistent/meeting"), cut)
                    .unwrap_err()
                    .contains("cut point")
            );
        }
    }

    #[test]
    fn rejects_missing_meeting() {
        assert!(copy_until(Path::new("/nonexistent/meeting"), 1_000_000).is_err());
    }
}

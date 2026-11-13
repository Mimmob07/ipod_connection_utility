use super::*;

#[derive(Clone, Debug, PartialEq)]
pub struct DownloadProgress {
    pub percent: Option<u8>,
    pub phase: &'static str,
    pub title: Option<String>,
}

impl DownloadProgress {
    pub(super) fn phase(phase: &'static str) -> Self {
        Self {
            percent: None,
            phase,
            title: None,
        }
    }
}

fn parse_line(line: &str) -> Option<DownloadProgress> {
    if line.contains("[download]") {
        let before = line.split_once('%')?.0;
        let number: f64 = before.split_whitespace().last()?.parse().ok()?;
        if number.is_finite() && (0.0..=100.0).contains(&number) {
            return Some(DownloadProgress {
                percent: Some(number as u8),
                phase: "Downloading",
                title: None,
            });
        }
    }
    if line.contains("[Track ")
        && let Some((_, title)) = line.split_once("Downloading \"")
    {
        let title = title.rsplit_once('"')?.0;
        return Some(DownloadProgress {
            percent: None,
            phase: "Downloading",
            title: Some(
                title
                    .chars()
                    .filter(|c| !c.is_control())
                    .take(300)
                    .collect(),
            ),
        });
    }
    None
}

pub(super) fn wait_download(
    child: &mut Child,
    cancelled: &AtomicBool,
    timeout: Duration,
    log_path: &Path,
    on_progress: &mut dyn FnMut(DownloadProgress),
) -> Result<ExitStatus, DownloadError> {
    let mut log = fs::File::open(log_path).map_err(|error| {
        DownloadError::Backend(format!("cannot follow download progress: {error}"))
    })?;
    let mut pending = Vec::new();
    let mut previous = None;
    wait_child_reporting(child, cancelled, timeout, &mut || {
        let mut bytes = Vec::new();
        Read::by_ref(&mut log)
            .take(64 * 1024)
            .read_to_end(&mut bytes)
            .map_err(|error| {
                DownloadError::Backend(format!("cannot read download progress: {error}"))
            })?;
        pending.extend(bytes);
        let mut consumed = 0;
        for (index, byte) in pending.iter().enumerate() {
            if *byte == b'\n' || *byte == b'\r' {
                if let Some(progress) =
                    parse_line(&String::from_utf8_lossy(&pending[consumed..index]))
                    && previous.as_ref() != Some(&progress)
                {
                    on_progress(progress.clone());
                    previous = Some(progress);
                }
                consumed = index + 1;
            }
        }
        pending.drain(..consumed);
        // Some downloaders flush an updating percentage without a trailing newline.
        if let Some(progress) = parse_line(&String::from_utf8_lossy(&pending))
            && previous.as_ref() != Some(&progress)
        {
            on_progress(progress.clone());
            previous = Some(progress);
        }
        if pending.len() > 16 * 1024 {
            pending.clear();
        }
        Ok(())
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_actual_percentages_and_titles_without_forwarding_raw_logs() {
        assert_eq!(
            parse_line("[download]  42.7% of ~ 4.00MiB")
                .unwrap()
                .percent,
            Some(42)
        );
        assert_eq!(
            parse_line("[INFO] [Track   1/1  ] Downloading \"Song Name\"")
                .unwrap()
                .title
                .as_deref(),
            Some("Song Name")
        );
        for other in [
            "token=secret",
            "[download] NaN%",
            "[download] 300%",
            "a title with 99%",
        ] {
            assert!(parse_line(other).is_none());
        }
    }

    #[test]
    fn follows_child_output_across_partial_progress_updates() {
        let fixture = crate::sync::tests::Fixture::new("download_progress_child");
        let path = fixture.ipod_file("progress.log");
        let output = fs::File::create(&path).unwrap();
        let mut child = Command::new("python3").arg("-c").arg(
            "import sys,time; print('[INFO] [Track 1/1] Downloading \"Test Song\"',flush=True); sys.stdout.write('[download] 4'); sys.stdout.flush(); time.sleep(0.15); sys.stdout.write('2.7% of 4MiB\\r[download] 100% of 4MiB\\n'); sys.stdout.flush()")
            .stdout(Stdio::from(output)).spawn().unwrap();
        let mut updates = Vec::new();
        assert!(
            wait_download(
                &mut child,
                &AtomicBool::new(false),
                Duration::from_secs(5),
                &path,
                &mut |update| updates.push(update)
            )
            .unwrap()
            .success()
        );
        assert!(
            updates
                .iter()
                .any(|update| update.title.as_deref() == Some("Test Song"))
        );
        assert!(updates.iter().any(|update| update.percent == Some(42)));
        assert!(updates.iter().any(|update| update.percent == Some(100)));
    }
}

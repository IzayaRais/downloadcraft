//! Plain-FTP backend (P3): `ftp://[user:pass@]host[:port]/path` downloads
//! with PASV binary transfers and REST resume, bound to the same `.dcpart`
//! + `.dcmeta` resume contract as HTTP (`RESUME` offsets from another URL
//! are discarded, never resumed into this file).
//!
//! Clean-room RFC 959 subset, zero new dependencies: control connection
//! (USER/PASS/TYPE I/SIZE/PASV/REST/RETR/QUIT) + one PASV data connection
//! per transfer. FTPS/SFTP are NOT attempted — an `ftps://` URL is
//! rejected loudly instead of silently downgrading to plaintext.
//!
//! Only `download_ftp()` is public; the tiny line protocol + fixture
//! server live here for unit tests.

use crate::engine::{io_message, mutex_guard, resume_offset, EngineOpts, RateLimiter, SharedState};
use anyhow::{anyhow, Result};
use std::sync::atomic::Ordering;
use std::sync::Arc;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt};

struct FtpTarget {
    host: String,
    port: u16,
    user: String,
    pass: String,
    path: String,
}

fn parse_target(url: &str) -> Result<FtpTarget> {
    if url.to_ascii_lowercase().starts_with("ftps://") {
        return Err(anyhow!(
            "FTPS (FTP over TLS) is not supported — use plain ftp:// or https://"
        ));
    }
    let u = url::Url::parse(url).map_err(|_| anyhow!("bad ftp URL: {url}"))?;
    if u.scheme() != "ftp" {
        return Err(anyhow!("not an ftp URL: {url}"));
    }
    let host = u.host_str().unwrap_or("").to_string();
    if host.is_empty() {
        return Err(anyhow!("ftp URL has no host: {url}"));
    }
    // The URL parser hands back percent-encoded text; the server wants the
    // real names (`ftp://h/My%20File.zip` is the file "My File.zip").
    Ok(FtpTarget {
        host,
        port: u.port().unwrap_or(21),
        user: if u.username().is_empty() {
            "anonymous".to_string()
        } else {
            percent_decode(u.username())
        },
        pass: u.password().map(percent_decode).unwrap_or_else(|| "anonymous@".to_string()),
        path: if u.path().is_empty() { "/".to_string() } else { percent_decode(u.path()) },
    })
}

/// `%XX` escapes only (a URL path keeps a literal `+`, unlike form data).
fn percent_decode(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%' && i + 2 < b.len() {
            if let Ok(v) = u8::from_str_radix(&s[i + 1..i + 3], 16) {
                out.push(v);
                i += 3;
                continue;
            }
        }
        out.push(b[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// Read one reply (single- or multi-line `code-…code …`) from the control
/// connection. Returns (code, full text).
async fn read_reply(
    r: &mut tokio::io::BufReader<tokio::net::tcp::OwnedReadHalf>,
) -> Result<(u32, String)> {
    let mut text = String::new();
    let mut code = 0u32;
    loop {
        let mut line = String::new();
        if r.read_line(&mut line).await.map_err(|e| anyhow!("ftp control: {e}"))? == 0 {
            return Err(anyhow!("ftp server closed the connection ({text})"));
        }
        text.push_str(line.trim_end());
        text.push(' ');
        if line.len() >= 4 {
            if let Ok(c) = line[..3].parse::<u32>() {
                code = code.max(c);
                if line.as_bytes().get(3) == Some(&b' ') {
                    break;
                }
            }
        }
        if text.len() > 16 * 1024 {
            return Err(anyhow!("ftp server sent an overlong reply"));
        }
    }
    Ok((code, text.trim().to_string()))
}

async fn cmd(
    w: &mut tokio::net::tcp::OwnedWriteHalf,
    r: &mut tokio::io::BufReader<tokio::net::tcp::OwnedReadHalf>,
    line: &str,
) -> Result<(u32, String)> {
    w.write_all(line.as_bytes()).await.map_err(|e| anyhow!(io_message(&e)))?;
    w.write_all(b"\r\n").await.map_err(|e| anyhow!(io_message(&e)))?;
    w.flush().await.map_err(|e| anyhow!(io_message(&e)))?;
    read_reply(r).await
}

fn parse_pasv(text: &str) -> Result<(String, u16)> {
    let s = text.find('(').and_then(|a| text[a + 1..].find(')').map(|b| text[a + 1..a + 1 + b].to_string()));
    let s = s.ok_or_else(|| anyhow!("ftp server sent an unparseable PASV reply"))?;
    let nums: Vec<u16> = s.split(',').filter_map(|x| x.trim().parse().ok()).collect();
    if nums.len() != 6 {
        return Err(anyhow!("ftp server sent an unparseable PASV reply"));
    }
    Ok((
        format!("{}.{}.{}.{}", nums[0], nums[1], nums[2], nums[3]),
        nums[4] * 256 + nums[5],
    ))
}

/// Download `url` to `dest` over plain FTP with REST resume. `limiter` is
/// the shared token bucket (global speed cap honored like HTTP legs).
pub async fn download_ftp(
    url: &str,
    dest: &std::path::PathBuf,
    opts: &EngineOpts,
    shared: Arc<SharedState>,
    limiter: Arc<RateLimiter>,
) -> Result<()> {
    let t = parse_target(url)?;
    if let Some(parent) = dest.parent() {
        if !parent.as_os_str().is_empty() {
            tokio::fs::create_dir_all(parent).await?;
        }
    }
    let connect_to = opts.timeout_secs.max(5).min(120);
    let stall_secs = opts.timeout_secs.max(15).min(120);
    let max_retries = opts.max_retries.max(1);

    // Control connection with a connect timeout (a blackhole host must not
    // hang the queue forever).
    let ctrl = tokio::time::timeout(
        std::time::Duration::from_secs(connect_to),
        tokio::net::TcpStream::connect((t.host.as_str(), t.port)),
    )
    .await
    .map_err(|_| anyhow!("ftp connect timed out ({connect_to}s) — resuming"))?
    .map_err(|e| anyhow!("ftp connect: {e}"))?;
    let (cr, cw) = ctrl.into_split();
    let mut r = tokio::io::BufReader::new(cr);
    let mut w = cw;

    let (code, text) = read_reply(&mut r).await?;
    if !(220..300).contains(&code) {
        return Err(anyhow!("ftp server refused ({code} {text})"));
    }
    let (code, _) = cmd(&mut w, &mut r, &format!("USER {}", t.user)).await?;
    if code == 331 || code == 332 {
        let (code, text) = cmd(&mut w, &mut r, &format!("PASS {}", t.pass)).await?;
        if !(200..300).contains(&code) {
            let _ = cmd(&mut w, &mut r, "QUIT").await;
            return Err(anyhow!("ftp login refused ({code} {text}) — check the username/password in the Add dialog"));
        }
    } else if !(200..300).contains(&code) {
        return Err(anyhow!("ftp login refused ({code}) — check the username/password"));
    }
    let (code, text) = cmd(&mut w, &mut r, "TYPE I").await?;
    if !(200..300).contains(&code) {
        return Err(anyhow!("ftp server refused binary mode ({code} {text})"));
    }
    // SIZE is optional (some servers hide it): unknown total still streams.
    let mut total: Option<u64> = None;
    let (code, text) = cmd(&mut w, &mut r, &format!("SIZE {}", t.path)).await?;
    if code == 213 {
        total = text.split_whitespace().last().and_then(|s| s.parse().ok());
    }
    if let Some(tot) = total {
        shared.total.store(tot, Ordering::SeqCst);
        shared.has_total.store(true, Ordering::SeqCst);
    }
    // Bind resume state BEFORE touching bytes (P0 contract shared w/ HTTP).
    let mut start_at = resume_offset(dest, "dcpart", url, total, 1);
    {
        let mut totals = mutex_guard(&shared.part_total);
        let mut dones = mutex_guard(&shared.part_done);
        totals.clear();
        dones.clear();
        totals.push((0, total.unwrap_or(0)));
        dones.push(start_at);
    }
    shared.downloaded.store(start_at, Ordering::SeqCst);

    let tmp = dest.with_extension("dcpart");
    let mut stalled = 0u32;
    // The "REST rejected → restart from zero" fallback runs once per job.
    let mut restarted_from_zero = false;
    let mut attempt = 0u32;
    loop {
        attempt += 1;
        if shared.cancelled.load(Ordering::SeqCst) {
            let _ = cmd(&mut w, &mut r, "QUIT").await;
            return Err(anyhow!("cancelled"));
        }
        crate::engine::wait_if_paused(&shared).await?;
        let len_before = tokio::fs::metadata(&tmp).await.map(|m| m.len()).unwrap_or(0);
        match transfer_once(&t, &mut w, &mut r, &tmp, len_before, stall_secs, &shared, &limiter).await {
            Ok(_) => {
                // Clean end-of-transfer: accept only when SIZE is met (or
                // unknown). A clean-but-short close resumes like a cut.
                let got = tokio::fs::metadata(&tmp).await.map(|m| m.len()).unwrap_or(0);
                if let Some(tot) = total {
                    if tot > 0 && got < tot {
                        if got > len_before {
                            stalled = 0;
                        } else {
                            stalled = stalled.saturating_add(1);
                        }
                        if stalled >= max_retries || attempt >= max_retries * 20 {
                            let _ = cmd(&mut w, &mut r, "QUIT").await;
                            return Err(anyhow!(
                                "connection ended early (got {got} of {tot} bytes) — resuming [{attempt} tries]"
                            ));
                        }
                        tokio::time::sleep(std::time::Duration::from_secs(1 << stalled.min(4))).await;
                        continue;
                    }
                }
                break;
            }
            Err(e) => {
                if shared.cancelled.load(Ordering::SeqCst) || e.to_string() == "cancelled" {
                    let _ = cmd(&mut w, &mut r, "QUIT").await;
                    return Err(e);
                }
                let msg = e.to_string();
                if msg.contains("REST rejected") {
                    // Server forgot our offset (or never resumes): one clean
                    // restart from zero, never a second.
                    // `start_at || len_before` alone let every later cut
                    // restart again (the restart writes bytes): a server that
                    // never resumes looped forever instead of failing.
                    if (start_at > 0 || len_before > 0) && !restarted_from_zero {
                        restarted_from_zero = true;
                        let _ = tokio::fs::remove_file(&tmp).await;
                        shared.downloaded.store(0, Ordering::SeqCst);
                        if let Ok(mut g) = shared.part_done.lock() {
                            if let Some(slot) = g.first_mut() {
                                *slot = 0;
                            }
                        }
                        start_at = 0;
                        stalled = 0;
                        // Rebind the sidecar to the fresh start.
                        crate::engine::resume_offset(dest, "dcpart", url, total, 1);
                        continue;
                    }
                    let _ = cmd(&mut w, &mut r, "QUIT").await;
                    return Err(e);
                }
                let len_after = tokio::fs::metadata(&tmp).await.map(|m| m.len()).unwrap_or(0);
                if len_after > len_before {
                    stalled = 0;
                } else {
                    stalled = stalled.saturating_add(1);
                }
                if stalled >= max_retries || attempt >= max_retries * 20 {
                    let _ = cmd(&mut w, &mut r, "QUIT").await;
                    return Err(anyhow!("{e:#} [{attempt} tries]"));
                }
                tokio::time::sleep(std::time::Duration::from_secs(1 << stalled.min(4))).await;
            }
        }
        // Completion check against SIZE when the server gave one.
        if let Some(tot) = total {
            let got = tokio::fs::metadata(&tmp).await.map(|m| m.len()).unwrap_or(0);
            if tot > 0 && got >= tot {
                break;
            }
            if tot == 0 {
                break;
            }
        }
    }
    let _ = cmd(&mut w, &mut r, "QUIT").await;
    tokio::fs::rename(&tmp, dest).await.map_err(|e| anyhow!(io_message(&e)))?;
    if !opts.expected_sha256.trim().is_empty() {
        if let Err(e) = crate::engine::verify_checksum(dest, &opts.expected_sha256).await {
            let _ = tokio::fs::remove_file(dest).await;
            return Err(e);
        }
    }
    Ok(())
}

/// One PASV+REST+RETR round ending in a clean transfer reply.
/// Mid-stream cuts surface as errors so the retry loop resumes them.
async fn transfer_once(
    t: &FtpTarget,
    w: &mut tokio::net::tcp::OwnedWriteHalf,
    r: &mut tokio::io::BufReader<tokio::net::tcp::OwnedReadHalf>,
    tmp: &std::path::PathBuf,
    offset: u64,
    stall_secs: u64,
    shared: &Arc<SharedState>,
    limiter: &Arc<RateLimiter>,
) -> Result<bool> {
    let (code, text) = cmd(w, r, "PASV").await?;
    if !(200..300).contains(&code) {
        return Err(anyhow!("ftp PASV refused ({code} {text})"));
    }
    let (ip, port) = parse_pasv(&text)?;
    // Open the data connection as soon as PASV names it, before RETR: some
    // servers (vsftpd, ProFTPD) wait for it before sending "150", while we
    // used to wait for "150" before connecting, so both sides waited.
    let mut data = tokio::net::TcpStream::connect((ip.as_str(), port))
        .await
        .map_err(|e| anyhow!("ftp data connect: {e}"))?;
    if offset > 0 {
        let (code, text) = cmd(w, r, &format!("REST {offset}")).await?;
        if !(300..400).contains(&code) {
            return Err(anyhow!("REST rejected ({code} {text}) — resuming from zero"));
        }
    }
    let (code, text) = cmd(w, r, &format!("RETR {}", t.path)).await?;
    if !(100..200).contains(&code) {
        return Err(anyhow!("ftp retrieve refused ({code} {text})"));
    }
    #[allow(clippy::suspicious_open_options)]
    let mut file = tokio::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(tmp)
        .await
        .map_err(|e| anyhow!(io_message(&e)))?;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let mut buf = [0u8; 32 * 1024];
    loop {
        crate::engine::wait_if_paused(shared).await?;
        if shared.cancelled.load(Ordering::SeqCst) {
            return Err(anyhow!("cancelled"));
        }
        let n = tokio::time::timeout(std::time::Duration::from_secs(stall_secs), data.read(&mut buf))
            .await
            .map_err(|_| anyhow!("stalled (no data for {stall_secs}s) — resuming"))?
            .map_err(|e| anyhow!("ftp data: {e}"))?;
        if n == 0 {
            break;
        }
        file.write_all(&buf[..n]).await.map_err(|e| anyhow!(io_message(&e)))?;
        shared.downloaded.fetch_add(n as u64, Ordering::SeqCst);
        if let Ok(mut g) = shared.part_done.lock() {
            if let Some(slot) = g.first_mut() {
                *slot = slot.saturating_add(n as u64);
            }
        }
        limiter.consume(n as u64).await;
    }
    file.flush().await.map_err(|e| anyhow!(io_message(&e)))?;
    drop(file);
    drop(data);
    // End-of-transfer reply (226/250/2xx), guarded so a silent server
    // can't hang the queue. A 426/550 here means the bytes above are
    // suspect — surface it instead of finalizing.
    let (code, text) = tokio::time::timeout(std::time::Duration::from_secs(stall_secs), read_reply(r))
        .await
        .map_err(|_| anyhow!("stalled (no end-of-transfer reply for {stall_secs}s) — resuming"))??;
    if (200..300).contains(&code) {
        return Ok(true);
    }
    Err(anyhow!("ftp transfer aborted ({code} {text}) — resuming"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn url_parts_are_percent_decoded() {
        let t = parse_target("ftp://us%40er:p%20w@h.example/My%20File+1.zip").unwrap();
        assert_eq!(t.user, "us@er");
        assert_eq!(t.pass, "p w");
        assert_eq!(t.path, "/My File+1.zip", "a URL path keeps a literal '+'");
        assert_eq!(percent_decode("bad%zz%4"), "bad%zz%4");
    }

    /// Minimal in-process FTP server (localhost only): one canned file,
    /// optional login gate, REST offsets, PASV data sends. Pending data
    /// listeners + REST offsets live in per-connection shared state.
    async fn spawn_ftp(body: Vec<u8>, want_user: Option<(String, String)>) -> std::net::SocketAddr {
        use tokio::io::{AsyncBufReadExt, AsyncWriteExt};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let body = Arc::new(body);
        tokio::spawn(async move {
            loop {
                let Ok((ctrl, _)) = listener.accept().await else { break };
                let body = body.clone();
                let want = want_user.clone();
                tokio::spawn(async move {
                    let (cr, cw) = ctrl.into_split();
                    let mut r = tokio::io::BufReader::new(cr);
                    let mut w = cw;
                    async fn send(
                        w: &mut tokio::net::tcp::OwnedWriteHalf,
                        s: &str,
                    ) {
                        use tokio::io::AsyncWriteExt;
                        let _ = w.write_all(s.as_bytes()).await;
                        let _ = w.flush().await;
                    }
                    send(&mut w, "220 dc-test ftp\r\n").await;
                    let pending: Arc<std::sync::Mutex<(Option<tokio::net::TcpListener>, u64)>> =
                        Arc::new(std::sync::Mutex::new((None, 0)));
                    let mut line = String::new();
                    loop {
                        line.clear();
                        if r.read_line(&mut line).await.unwrap_or(0) == 0 {
                            break;
                        }
                        let t = line.trim().to_string();
                        let (verb, arg) = t.split_once(' ').map(|(a, b)| (a.to_ascii_uppercase(), b.to_string())).unwrap_or((t.to_ascii_uppercase(), String::new()));
                        match verb.as_str() {
                            "USER" => {
                                if let Some((u, _)) = &want {
                                    if arg != *u {
                                        send(&mut w, "530 bad user\r\n").await;
                                        continue;
                                    }
                                }
                                send(&mut w, "331 ok, need pass\r\n").await;
                            }
                            "PASS" => {
                                if let Some((_, p)) = &want {
                                    if arg != *p {
                                        send(&mut w, "530 login incorrect\r\n").await;
                                        continue;
                                    }
                                }
                                send(&mut w, "230 logged in\r\n").await;
                            }
                            "TYPE" => send(&mut w, "200 type set\r\n").await,
                            "SIZE" => {
                                if arg == "/file.bin" {
                                    send(&mut w, &format!("213 {}\r\n", body.len())).await;
                                } else {
                                    send(&mut w, "550 not found\r\n").await;
                                }
                            }
                            "PASV" => {
                                match tokio::net::TcpListener::bind("127.0.0.1:0").await {
                                    Ok(dl) => {
                                        let p = dl.local_addr().unwrap().port();
                                        if let Ok(mut g) = pending.lock() {
                                            g.0 = Some(dl);
                                        }
                                        send(&mut w, &format!("227 Entering Passive Mode (127,0,0,1,{}, {})\r\n", p / 256, p % 256)).await;
                                    }
                                    Err(_) => send(&mut w, "425 no data conn\r\n").await,
                                }
                            }
                            "REST" => {
                                match arg.parse::<u64>() {
                                    Ok(n) => {
                                        if let Ok(mut g) = pending.lock() {
                                            g.1 = n;
                                        }
                                        send(&mut w, "350 restart at offset\r\n").await;
                                    }
                                    Err(_) => send(&mut w, "501 bad offset\r\n").await,
                                }
                            }
                            "RETR" => {
                                if arg != "/file.bin" {
                                    send(&mut w, "550 not found\r\n").await;
                                    continue;
                                }
                                let (dl, off) = match pending.lock().map(|mut g| (g.0.take(), g.1)) {
                                    Ok(v) => v,
                                    Err(_) => (None, 0),
                                };
                                let Some(dl) = dl else {
                                    send(&mut w, "425 use PASV first\r\n").await;
                                    continue;
                                };
                                let body = body.clone();
                                tokio::spawn(async move {
                                    if let Ok((mut ds, _)) = dl.accept().await {
                                        let _ = ds.write_all(&body[off.min(body.len() as u64) as usize..]).await;
                                    }
                                });
                                send(&mut w, "150 opening data\r\n").await;
                                // Give the data connection time to deliver.
                                tokio::time::sleep(std::time::Duration::from_millis(400)).await;
                                send(&mut w, "226 done\r\n").await;
                            }
                            "QUIT" => {
                                send(&mut w, "221 bye\r\n").await;
                                break;
                            }
                            _ => send(&mut w, "502 nah\r\n").await,
                        }
                    }
                });
            }
        });
        addr
    }

    fn ftp_opts() -> EngineOpts {
        EngineOpts { timeout_secs: 10, max_retries: 3, user_agent: "dc-test".into(), ..Default::default() }
    }

    async fn dl(url: &str, dest: &std::path::PathBuf) -> Result<()> {
        let shared = Arc::new(SharedState::new());
        let lim = Arc::new(RateLimiter::new(0));
        download_ftp(url, dest, &ftp_opts(), shared, lim).await
    }

    #[tokio::test]
    async fn ftp_full_and_resume() {
        let body: Vec<u8> = (0..200_000).map(|i| (i % 251) as u8).collect();
        let addr = spawn_ftp(body.clone(), None).await;
        let dir = std::env::temp_dir().join(format!("dc_ftp_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let url = format!("ftp://{addr}/file.bin");
        // 1. Full download, byte-exact.
        let dest = dir.join("full.bin");
        dl(&url, &dest).await.expect("ftp full");
        assert_eq!(std::fs::read(&dest).unwrap(), body);
        // 2. Resume with a bound prefix continues, not restarts.
        let dest = dir.join("resume.bin");
        std::fs::write(dest.with_extension("dcpart"), &body[..50_000]).unwrap();
        dl(&url, &dest).await.expect("ftp resume");
        assert_eq!(std::fs::read(&dest).unwrap(), body);
        // 3. Stale prefix bound to another URL is discarded, never resumed
        // (garbage prefix + real suffix would corrupt the file).
        let dest = dir.join("stale.bin");
        std::fs::write(dest.with_extension("dcpart"), vec![0xFFu8; 50_000]).unwrap();
        let foreign = format!("ftp://{addr}/foreign.bin");
        crate::engine::resume_offset(&dest, "dcpart", &foreign, Some(body.len() as u64), 1);
        dl(&url, &dest).await.expect("ftp stale discard");
        assert_eq!(std::fs::read(&dest).unwrap(), body);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn ftp_login_refused() {
        let body: Vec<u8> = (0..1000).map(|i| (i % 251) as u8).collect();
        let addr = spawn_ftp(body, Some(("tester".into(), "s3cret".into()))).await;
        let dir = std::env::temp_dir().join(format!("dc_ftpauth_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        // Wrong password → loud login error, no retry storm, no file.
        let dest = dir.join("auth.bin");
        let err = dl(&format!("ftp://bob:wrong@{addr}/file.bin"), &dest).await.expect_err("bad login");
        assert!(format!("{err:#}").contains("login"), "got: {err:#}");
        assert!(!dest.exists());
        // Right credentials → exact file.
        let dest = dir.join("auth2.bin");
        dl(&format!("ftp://tester:s3cret@{addr}/file.bin"), &dest).await.expect("good login");
        assert_eq!(std::fs::read(&dest).unwrap().len(), 1000);
        let _ = std::fs::remove_dir_all(&dir);
    }
}

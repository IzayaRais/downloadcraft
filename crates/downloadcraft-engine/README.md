# downloadcraft-engine

The download engine behind [Download Craft](https://github.com/hungry-detective/downloadcraft), as a
standalone Rust crate you can drop into your own downloader. MIT licensed, async (`tokio` +
`reqwest`).

## Why it is fast

Most downloaders split a file into N equal parts once. If one connection is slow, the others finish and
**sit idle**, and the whole download waits for the slowest part. This engine never lets a connection
idle while there is work left.

| | Fixed split | downloadcraft-engine |
|---|---|---|
| A connection finishes early | It stops; the others carry on alone | It takes over half of whatever will finish last |
| One connection is slow | The download waits for it | Its part shrinks, others take the rest; very slow connections are reopened |
| Starting up | HEAD/probe request, then N requests | The first request downloads immediately and doubles as the probe |
| Finishing | Join N part files (every byte copied again) | One pre-allocated file, renamed when done |
| Crash / power cut | Resume per part, if the split stayed the same | Journal of missing ranges, flushed every 3 s; any number of connections can continue it |
| Server limits connections | Errors | 429/503 halves the connections; a per-IP limit (403 on an extra connection) is detected |
| Signed link expires | Fails | Retries via the original URL, then your refresh hook; keeps every byte |
| Server sends an error page | Saved as `movie.mp4` | Refused (`Error::HtmlPage`) |
| File changed on the server | Old and new bytes stitched together | Size/ETag/Last-Modified checked; starts over cleanly |

Measured on a home connection (100 MB, 8 connections, byte-identical results) against Download
Craft 0.9.8's fixed-split engine:

| Server | Fixed split | This engine |
|---|---|---|
| speedtest.belwue.net | 70.6 s | 21.4 s |
| mirror.nforce.com | 28.2 s | 19.7 s |
| thinkbroadband.com | 26.9 s | 27.3 s (both at the line's limit) |

The test suite also includes a local server whose first connection crawls at 64 KiB/s. The engine
finishes the 8 MiB file in seconds; a fixed 4-way split would need about 32 s for that connection's
quarter alone.

## Use it

```toml
[dependencies]
downloadcraft-engine = { git = "https://github.com/hungry-detective/downloadcraft" }
tokio = { version = "1", features = ["full"] }
reqwest = "0.12"
```

```rust
use downloadcraft_engine::{Config, Control, Downloader, Job, Progress};
use std::sync::Arc;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let progress = Arc::new(Progress::default());
    let control = Arc::new(Control::default()); // control.pause() / resume() / cancel()

    let downloader = Downloader::new(reqwest::Client::new())
        .config(Config { connections: 8, ..Config::default() });

    let job = Job::new("https://example.com/big.iso", "big.iso");
    let done = downloader.download(job, progress.clone(), control).await?;
    println!("{} bytes, {} connections at peak", done.size, done.peak_connections);
    Ok(())
}
```

`cargo run --example get -- <url> [file]` runs a complete command-line downloader with a live
progress line.

### Hooks

- `Downloader::with_request(Arc::new(|url| client.get(url).header(...)))`: build every request
  yourself (cookies, auth, referer, custom headers). The engine adds `Range`.
- `.limiter(Arc<RateLimiter>)`: a speed limit, shared across downloads if you like, adjustable live.
- `.connection_gate(Arc<Semaphore>)`: cap the connections to one server across many downloads.
- `.on_link_expired(...)`: return a fresh URL for the same file when the link dies mid-download (for
  example, resolve the video page again). The engine checks that the size still matches.
- `.on_event(...)`: connections opened, splits, retries, reduced connections, refreshed links.

### Progress

`Progress` is readable from any thread: `downloaded()`, `total()`, `connections()`, `resumable()`, and
`segments()`, which lists every segment's start, end, bytes done and whether it is active. That is
enough to draw an IDM-style segment bar.

### Resume

While downloading, data is in `<file>.dcdata` and the resume journal in `<file>.dcjournal`. Running
the same `Job` again resumes it. If the URL changes between runs, as signed links do, give the job a
stable `resume_key`. `has_partial(path)` and `remove_partial(path)` inspect and clear that state.

### Extras

- `zip::list_remote(request, url)` lists a remote ZIP's files with one or two small range requests,
  without downloading the archive.
- `sniff::sniff(bytes)` recognises common formats from their first bytes, and
  `sniff::conflicts(ext, format)` tells you when a `.mp4` is really a ZIP or an HTML page.

## Tests

```
cargo test -p downloadcraft-engine
```

The unit tests cover the segment map, journal, file writes, sniffing and ZIP parsing. The integration
tests use a misbehaving local server: a slow first connection, cut responses, 429 throttling,
expiring signed redirects, a dead link with a refresh hook, a server without ranges, a 1 MiB range
limit, an HTML error page, and a file that changes between runs.

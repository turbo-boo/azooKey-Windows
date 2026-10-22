//! End-to-end smoke test and latency probe for a built Azookey.
//!
//! Talks to azookey-server and ui.exe over their named pipes the way the IME
//! does. With `--start <dir>` it first runs `<dir>\launcher.exe` and waits for
//! both pipes. Exits non-zero when a check fails. Run it on a disposable
//! Windows machine (CI): it drives the shared converter state.

use std::{
    future::Future,
    path::PathBuf,
    time::{Duration, Instant},
};

use anyhow::{bail, ensure, Context, Result};
use hyper_util::rt::TokioIo;
use shared::proto::{
    azookey_service_client::AzookeyServiceClient, window_service_client::WindowServiceClient,
    AppendTextRequest, ClearTextRequest, EmptyResponse, MoveCursorRequest, RequestConversionRequest,
    SetCandidateRequest, SetInputModeRequest, SetSelectionRequest, ShrinkTextRequest,
};
use tonic::transport::{Channel, Endpoint};
use tower::service_fn;

mod pipe_acl;
mod wakeup_probe;

const SERVER_PIPE: &str = r"\\.\pipe\azookey_server";
const UI_PIPE: &str = r"\\.\pipe\azookey_ui";
const ERROR_PIPE_BUSY: i32 = 231;
const CALL_TIMEOUT: Duration = Duration::from_secs(30);

struct Args {
    start: Option<PathBuf>,
    keys: String,
}

fn parse_args() -> Result<Args> {
    let mut args = Args {
        start: None,
        keys: "kyouhaiitenkidesune".to_string(),
    };
    let mut iter = std::env::args().skip(1);
    while let Some(arg) = iter.next() {
        match arg.as_str() {
            "--start" => args.start = Some(iter.next().context("--start needs a directory")?.into()),
            "--keys" => args.keys = iter.next().context("--keys needs romaji")?,
            other => bail!("unknown argument: {other}"),
        }
    }
    Ok(args)
}

async fn connect(pipe: &'static str) -> Result<Channel> {
    let channel = Endpoint::try_from("http://[::]:50051")?
        .connect_with_connector(service_fn(move |_| async move {
            loop {
                match shared::pipe::open_client(pipe) {
                    Ok(client) => return Ok::<_, std::io::Error>(TokioIo::new(client)),
                    Err(e) if e.raw_os_error() == Some(ERROR_PIPE_BUSY) => {
                        tokio::time::sleep(Duration::from_millis(50)).await
                    }
                    Err(e) => return Err(e),
                }
            }
        }))
        .await?;
    Ok(channel)
}

async fn wait_for(pipe: &'static str, within: Duration) -> Result<Channel> {
    let deadline = Instant::now() + within;
    loop {
        match connect(pipe).await {
            Ok(channel) => return Ok(channel),
            Err(error) if Instant::now() < deadline => {
                let _ = error;
                tokio::time::sleep(Duration::from_millis(500)).await;
            }
            Err(error) => return Err(error).with_context(|| format!("{pipe} did not come up")),
        }
    }
}

/// Runs one RPC with a timeout and records how long it took.
async fn timed<T>(
    samples: &mut Vec<Duration>,
    call: impl Future<Output = Result<tonic::Response<T>, tonic::Status>>,
) -> Result<T> {
    let start = Instant::now();
    let response = tokio::time::timeout(CALL_TIMEOUT, call)
        .await
        .context("RPC timed out")??;
    samples.push(start.elapsed());
    Ok(response.into_inner())
}

fn report(name: &str, samples: &[Duration]) {
    if samples.is_empty() {
        return;
    }
    let mut ms: Vec<f64> = samples.iter().map(|d| d.as_secs_f64() * 1000.0).collect();
    ms.sort_by(f64::total_cmp);
    let pick = |q: f64| ms[((ms.len() - 1) as f64 * q).round() as usize];
    println!(
        "{name:<18} n={:<3} p50={:>8.2}ms p95={:>8.2}ms max={:>8.2}ms",
        ms.len(),
        pick(0.5),
        pick(0.95),
        pick(1.0)
    );
}

async fn check_converter(client: &mut AzookeyServiceClient<Channel>, keys: &str) -> Result<()> {
    let mut append = Vec::new();
    let mut convert = Vec::new();
    let mut other = Vec::new();

    timed(&mut other, client.clear_text(ClearTextRequest {})).await?;

    // A single typed character is shown as is, never replaced by a
    // prediction such as と -> とか (azooKey-Desktop behaviour).
    let mut single = None;
    for key in ["t", "o"] {
        single = timed(
            &mut append,
            client.append_text(AppendTextRequest {
                text_to_append: key.to_string(),
            }),
        )
        .await?
        .composing_text;
    }
    let single = single.context("AppendText returned no composing text")?;
    let shown: Vec<&str> = single.suggestions.iter().map(|s| s.text.as_str()).collect();
    ensure!(
        single.hiragana == "と" && !single.is_converted && shown == ["と"],
        "typing \"to\" showed {shown:?} (converted: {}), expected [\"と\"]",
        single.is_converted
    );
    timed(&mut other, client.clear_text(ClearTextRequest {})).await?;

    // typing, as the IME sends it in kana mode
    let mut last = None;
    for key in keys.chars() {
        let response = timed(
            &mut append,
            client.append_text(AppendTextRequest {
                text_to_append: key.to_string(),
            }),
        )
        .await?;
        last = response.composing_text;
    }
    let typed = last.context("AppendText returned no composing text")?;
    ensure!(!typed.hiragana.is_empty(), "typing produced no hiragana");
    println!("typed {keys:?} -> {:?}", typed.hiragana);

    // Space: explicit conversion
    let converted = timed(&mut convert, client.request_conversion(RequestConversionRequest {}))
        .await?
        .composing_text
        .context("RequestConversion returned no composing text")?;
    ensure!(
        !converted.suggestions.is_empty(),
        "conversion of {:?} returned no candidates",
        converted.hiragana
    );
    println!("converted -> {:?}", converted.suggestions[0].text);
    // reported before the checks below so a failure there keeps the timings
    report("AppendText", &append);
    report("RequestConversion", &convert);

    // Committing a prefix longer than 127 characters must not wrap the count.
    timed(&mut other, client.clear_text(ClearTextRequest {})).await?;
    let long = "a".repeat(130);
    let before = timed(
        &mut other,
        client.append_text(AppendTextRequest {
            text_to_append: long,
        }),
    )
    .await?
    .composing_text
    .context("AppendText returned no composing text")?;
    ensure!(
        before.hiragana.chars().count() == 130,
        "expected 130 kana, got {}",
        before.hiragana.chars().count()
    );
    let after = timed(&mut other, client.shrink_text(ShrinkTextRequest { offset: 128 }))
        .await?
        .composing_text
        .context("ShrinkText returned no composing text")?;
    ensure!(
        after.hiragana.chars().count() == 2,
        "ShrinkText(128) left {} of 130 kana, expected 2",
        after.hiragana.chars().count()
    );

    // A negative count is rejected instead of crashing the converter.
    let negative = client.shrink_text(ShrinkTextRequest { offset: -1 }).await;
    ensure!(
        matches!(&negative, Err(status) if status.code() == tonic::Code::InvalidArgument),
        "ShrinkText(-1) returned {negative:?}, expected InvalidArgument"
    );
    let still = timed(&mut other, client.move_cursor(MoveCursorRequest { offset: 0 }))
        .await?
        .composing_text
        .context("MoveCursor returned no composing text")?;
    ensure!(
        still.hiragana.chars().count() == 2,
        "composition changed after rejected ShrinkText(-1): {:?}",
        still.hiragana
    );

    timed(&mut other, client.clear_text(ClearTextRequest {})).await?;
    report("other server RPCs", &other);
    Ok(())
}

async fn check_window(client: &mut WindowServiceClient<Channel>) -> Result<()> {
    // More updates than the UI's internal queue holds (32): if the webviews
    // failed to start, the queue is never drained and these time out.
    let mut samples = Vec::new();
    let candidates: Vec<String> = ["今日", "京", "強"].iter().map(|s| s.to_string()).collect();
    for round in 0..20 {
        timed(
            &mut samples,
            client.set_candidate(SetCandidateRequest {
                candidates: candidates.clone(),
            }),
        )
        .await
        .with_context(|| format!("SetCandidate #{round}"))?;
        timed(&mut samples, client.set_selection(SetSelectionRequest { index: round % 3 }))
            .await
            .with_context(|| format!("SetSelection #{round}"))?;
        timed(&mut samples, client.show_window(EmptyResponse {})).await?;
        let mode = if round % 2 == 0 { "あ" } else { "A" };
        timed(
            &mut samples,
            client.set_input_mode(SetInputModeRequest {
                mode: mode.to_string(),
            }),
        )
        .await?;
    }
    timed(&mut samples, client.hide_window(EmptyResponse {})).await?;
    report("window RPCs", &samples);
    Ok(())
}

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<()> {
    let args = parse_args()?;

    if let Some(dir) = &args.start {
        use std::os::windows::process::CommandExt as _;
        // The startup task runs launcher.exe at Task Scheduler priority 7,
        // i.e. BELOW_NORMAL_PRIORITY_CLASS; start it the same way.
        const BELOW_NORMAL_PRIORITY_CLASS: u32 = 0x0000_4000;
        let launcher = dir.join("launcher.exe");
        std::process::Command::new(&launcher)
            .current_dir(dir)
            .creation_flags(BELOW_NORMAL_PRIORITY_CLASS)
            .spawn()
            .with_context(|| format!("failed to start {}", launcher.display()))?;
    }

    let server = wait_for(SERVER_PIPE, Duration::from_secs(180)).await?;
    let window = wait_for(UI_PIPE, Duration::from_secs(60)).await?;
    println!("connected to {SERVER_PIPE} and {UI_PIPE}");

    let mut failures = Vec::new();
    for pipe in [SERVER_PIPE, UI_PIPE] {
        if let Err(error) = pipe_acl::check_clients_cannot_create_instances(pipe) {
            failures.push(format!("{pipe} ACL: {error:#}"));
        }
    }
    let idle = wakeup_probe::WakeupProbe::start();
    tokio::time::sleep(Duration::from_secs(2)).await;
    report("wakeup lateness (idle)", &idle.finish());

    let busy = wakeup_probe::WakeupProbe::start();
    let converter = check_converter(&mut AzookeyServiceClient::new(server), &args.keys).await;
    report("wakeup lateness (converting)", &busy.finish());
    if let Err(error) = converter {
        failures.push(format!("converter: {error:#}"));
    }
    if let Err(error) = check_window(&mut WindowServiceClient::new(window)).await {
        failures.push(format!("candidate window: {error:#}"));
    }

    if failures.is_empty() {
        println!("smoke test passed");
        Ok(())
    } else {
        for failure in &failures {
            eprintln!("FAILED {failure}");
        }
        bail!("{} smoke check(s) failed", failures.len())
    }
}

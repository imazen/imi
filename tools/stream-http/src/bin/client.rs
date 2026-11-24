//! Load client: for every (codec, size, mode, concurrency) it sends `n`
//! requests (after 2 warm-ups) and prints one TSV row: time to first body
//! byte and to the last byte (p50/p95, ms), output bytes, and throughput.
//!
//! `client <base-url> <label> <src> <codecs|all> <WxH,...> <stream,buffer> <conc,...> <n>`

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use bytes::Bytes;
use http_body_util::{BodyExt, Empty};
use hyper_util::client::legacy::Client;
use hyper_util::rt::TokioExecutor;
use imij_stream_http::{Codec, Error};

type Http = Client<hyper_util::client::legacy::connect::HttpConnector, Empty<Bytes>>;

struct Sample {
    ttfb: Duration,
    total: Duration,
    bytes: u64,
}

async fn one(client: &Http, url: &str) -> Result<Sample, Error> {
    let t0 = Instant::now();
    let resp = client.get(url.parse()?).await?;
    if !resp.status().is_success() {
        let body = resp.into_body().collect().await?.to_bytes();
        return Err(format!("{}: {}", url, String::from_utf8_lossy(&body)).into());
    }
    let mut body = resp.into_body();
    let mut ttfb = None;
    let mut bytes = 0u64;
    while let Some(frame) = body.frame().await {
        if let Ok(data) = frame?.into_data() {
            if !data.is_empty() {
                ttfb.get_or_insert_with(|| t0.elapsed());
                bytes += data.len() as u64;
            }
        }
    }
    Ok(Sample {
        ttfb: ttfb.unwrap_or_default(),
        total: t0.elapsed(),
        bytes,
    })
}

fn pct(v: &mut [Duration], p: f64) -> f64 {
    v.sort();
    let i = ((v.len() as f64 - 1.0) * p).round() as usize;
    v[i].as_secs_f64() * 1e3
}

#[tokio::main]
async fn main() -> Result<(), Error> {
    let a: Vec<String> = std::env::args().skip(1).collect();
    if a.len() != 8 {
        return Err("usage: client <base-url> <label> <src> <codecs|all> <WxH,...> <stream,buffer> <conc,...> <n>".into());
    }
    let (base, label, src) = (&a[0], &a[1], &a[2]);
    let codecs: Vec<Codec> = if a[3] == "all" {
        Codec::ALL.to_vec()
    } else {
        a[3].split(',')
            .map(|c| Codec::parse(c).ok_or(format!("codec {c}")))
            .collect::<Result<_, _>>()?
    };
    let sizes: Vec<(u32, u32)> = a[4]
        .split(',')
        .map(|s| {
            let (w, h) = s.split_once('x').ok_or("size WxH")?;
            Ok::<_, Error>((w.parse()?, h.parse()?))
        })
        .collect::<Result<_, _>>()?;
    let modes: Vec<&str> = a[5].split(',').collect();
    let concs: Vec<usize> = a[6]
        .split(',')
        .map(|c| c.parse())
        .collect::<Result<_, _>>()?;
    let n: usize = a[7].parse()?;
    let client: Http = Client::builder(TokioExecutor::new())
        .pool_max_idle_per_host(64)
        .build_http();
    println!(
        "label\tsrc\tcodec\tsize\tmode\tconc\tn\tbytes\tttfb_p50_ms\tttfb_p95_ms\ttotal_p50_ms\ttotal_p95_ms\twall_s\tMB_per_s\tMpx_per_s"
    );
    for &codec in &codecs {
        for &(w, h) in &sizes {
            for mode in &modes {
                for &conc in &concs {
                    let url = Arc::new(format!(
                        "{base}/enc?codec={}&w={w}&h={h}&mode={mode}&src={src}",
                        codec.name()
                    ));
                    for _ in 0..2 {
                        one(&client, &url).await?;
                    }
                    let next = Arc::new(AtomicUsize::new(0));
                    let t0 = Instant::now();
                    let mut tasks = Vec::new();
                    for _ in 0..conc {
                        let (client, url, next) = (client.clone(), url.clone(), next.clone());
                        tasks.push(tokio::spawn(async move {
                            let mut out = Vec::new();
                            while next.fetch_add(1, Ordering::Relaxed) < n {
                                out.push(one(&client, &url).await?);
                            }
                            Ok::<_, Error>(out)
                        }));
                    }
                    let mut samples = Vec::new();
                    for t in tasks {
                        samples.extend(t.await??);
                    }
                    let wall = t0.elapsed().as_secs_f64();
                    let bytes = samples[0].bytes;
                    let total_bytes: u64 = samples.iter().map(|s| s.bytes).sum();
                    let mut ttfb: Vec<Duration> = samples.iter().map(|s| s.ttfb).collect();
                    let mut total: Vec<Duration> = samples.iter().map(|s| s.total).collect();
                    println!(
                        "{label}\t{src}\t{}\t{w}x{h}\t{mode}\t{conc}\t{}\t{bytes}\t{:.3}\t{:.3}\t{:.3}\t{:.3}\t{wall:.3}\t{:.1}\t{:.1}",
                        codec.name(),
                        samples.len(),
                        pct(&mut ttfb, 0.5),
                        pct(&mut ttfb, 0.95),
                        pct(&mut total, 0.5),
                        pct(&mut total, 0.95),
                        total_bytes as f64 / wall / 1e6,
                        (w as f64 * h as f64 * samples.len() as f64) / wall / 1e6,
                    );
                }
            }
        }
    }
    Ok(())
}

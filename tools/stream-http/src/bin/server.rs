//! HTTP server: `GET /enc?codec=<name>&w=<px>&h=<px>&mode=stream|buffer&strip=<rows>&src=photo|noise`.
//!
//! `stream`: the encode runs on a blocking thread and every pending byte goes
//! to the response as soon as `pending_output` has it (chunked transfer,
//! a bounded channel for backpressure). `buffer`: one-shot `encode`, then the
//! whole file with a `Content-Length`. Usage: `server <port> <photo>`.

use std::convert::Infallible;
use std::sync::Arc;

use bytes::Bytes;
use http_body_util::{BodyExt, Full, StreamBody, combinators::BoxBody};
use hyper::body::Frame;
use hyper::{Request, Response, StatusCode};
use hyper_util::rt::TokioIo;
use imij_stream_http::{Codec, Error, Source, one_shot, stream_encode};
use tokio_stream::StreamExt;
use tokio_stream::wrappers::ReceiverStream;

type Body = BoxBody<Bytes, Error>;

struct State {
    photo: Source,
    noise: Source,
}

fn reply(status: StatusCode, msg: String) -> Response<Body> {
    Response::builder()
        .status(status)
        .body(Full::new(Bytes::from(msg)).map_err(|n| match n {}).boxed())
        .unwrap()
}

async fn handle(
    req: Request<hyper::body::Incoming>,
    st: Arc<State>,
) -> Result<Response<Body>, Infallible> {
    let q: std::collections::HashMap<String, String> = req
        .uri()
        .query()
        .unwrap_or("")
        .split('&')
        .filter_map(|kv| kv.split_once('='))
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect();
    let num = |k: &str, d: u32| q.get(k).and_then(|v| v.parse().ok()).unwrap_or(d);
    let Some(codec) = q.get("codec").and_then(|c| Codec::parse(c)) else {
        return Ok(reply(StatusCode::BAD_REQUEST, "unknown codec".into()));
    };
    let strip = num("strip", 16).max(1);
    let stream = q.get("mode").is_none_or(|m| m == "stream");
    let noise = q.get("src").is_some_and(|s| s == "noise");
    let size = source(&st, noise).crop(num("w", 1024), num("h", 1024));
    if stream {
        let (tx, rx) = tokio::sync::mpsc::channel::<Result<Bytes, Error>>(8);
        let st2 = st.clone();
        tokio::task::spawn_blocking(move || {
            let src = source(&st2, noise);
            let mut sink = |b: &[u8]| -> Result<(), Error> {
                tx.blocking_send(Ok(Bytes::copy_from_slice(b)))
                    .map_err(|_| Error::from("client went away"))
            };
            if let Err(e) = stream_encode(codec, src, size, strip, &mut sink) {
                eprintln!("{} {}x{} stream: {e}", codec.name(), size.0, size.1);
                let _ = tx.blocking_send(Err(e));
            }
        });
        let body = StreamBody::new(ReceiverStream::new(rx).map(|r| r.map(Frame::data)));
        Ok(Response::builder()
            .header("content-type", codec.mime())
            .body(BodyExt::boxed(body))
            .unwrap())
    } else {
        let st2 = st.clone();
        let res =
            tokio::task::spawn_blocking(move || one_shot(codec, source(&st2, noise), size)).await;
        match res {
            Ok(Ok(data)) => Ok(Response::builder()
                .header("content-type", codec.mime())
                .header("content-length", data.len())
                .body(Full::new(Bytes::from(data)).map_err(|n| match n {}).boxed())
                .unwrap()),
            Ok(Err(e)) => Ok(reply(StatusCode::INTERNAL_SERVER_ERROR, e.to_string())),
            Err(e) => Ok(reply(StatusCode::INTERNAL_SERVER_ERROR, e.to_string())),
        }
    }
}

/// The source a request reads.
fn source(st: &State, noise: bool) -> &Source {
    if noise { &st.noise } else { &st.photo }
}

#[tokio::main]
async fn main() -> Result<(), Error> {
    let mut args = std::env::args().skip(1);
    let port: u16 = args.next().ok_or("usage: server <port> <photo>")?.parse()?;
    let photo = Source::load(std::path::Path::new(
        &args.next().ok_or("usage: server <port> <photo>")?,
    ))?;
    let noise = Source::noise(photo.width, photo.height);
    eprintln!(
        "photo {}x{}; listening on 0.0.0.0:{port}",
        photo.width, photo.height
    );
    let st = Arc::new(State { photo, noise });
    let listener = tokio::net::TcpListener::bind(("0.0.0.0", port)).await?;
    loop {
        let (sock, _) = listener.accept().await?;
        sock.set_nodelay(true)?;
        let st = st.clone();
        tokio::spawn(async move {
            let svc = hyper::service::service_fn(move |req| handle(req, st.clone()));
            let _ = hyper::server::conn::http1::Builder::new()
                .serve_connection(TokioIo::new(sock), svc)
                .await;
        });
    }
}

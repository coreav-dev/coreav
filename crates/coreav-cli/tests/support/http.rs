//! A small HTTP/1.1 server for one object on 127.0.0.1, for the tests of
//! URL inputs.
#![allow(dead_code)]

use std::io::{BufRead, BufReader, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

#[derive(Clone, Copy, PartialEq)]
pub enum Mode {
    /// Answers ranges with 206.
    Ranges,
    /// Ignores ranges: 200 with the whole object.
    Whole,
    /// 404 for everything.
    Missing,
    /// Redirects to the same path over plain http.
    RedirectToHttp,
}

/// A small HTTP/1.1 server for one object, on 127.0.0.1.
pub struct Server {
    pub port: u16,
    pub requests: Arc<AtomicUsize>,
    pub etag: Arc<Mutex<String>>,
}

impl Server {
    pub fn start(data: Vec<u8>, mode: Mode) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let requests = Arc::new(AtomicUsize::new(0));
        let etag = Arc::new(Mutex::new("\"v1\"".to_owned()));
        let data = Arc::new(data);
        {
            let requests = Arc::clone(&requests);
            let etag = Arc::clone(&etag);
            std::thread::spawn(move || {
                for stream in listener.incoming() {
                    let Ok(stream) = stream else { continue };
                    let data = Arc::clone(&data);
                    let requests = Arc::clone(&requests);
                    let etag = Arc::clone(&etag);
                    std::thread::spawn(move || serve(stream, &data, mode, &requests, &etag, port));
                }
            });
        }
        Self {
            port,
            requests,
            etag,
        }
    }

    pub fn url(&self) -> String {
        format!("http://127.0.0.1:{}/media/file.mp4", self.port)
    }
}

fn serve(
    stream: TcpStream,
    data: &[u8],
    mode: Mode,
    requests: &AtomicUsize,
    etag: &Mutex<String>,
    port: u16,
) {
    let mut reader = BufReader::new(stream.try_clone().unwrap());
    let mut out = stream;
    loop {
        let mut line = String::new();
        if reader.read_line(&mut line).unwrap_or(0) == 0 {
            return;
        }
        let mut range = None;
        let mut if_match = None;
        loop {
            let mut header = String::new();
            if reader.read_line(&mut header).unwrap_or(0) == 0 {
                return;
            }
            let header = header.trim_end();
            if header.is_empty() {
                break;
            }
            let (name, value) = header.split_once(':').unwrap();
            match name.to_ascii_lowercase().as_str() {
                "range" => range = Some(value.trim().to_owned()),
                "if-match" => if_match = Some(value.trim().to_owned()),
                _ => {}
            }
        }
        requests.fetch_add(1, Ordering::SeqCst);
        let tag = etag.lock().unwrap().clone();
        let head = |status: &str, extra: &str, length: usize| {
            format!("HTTP/1.1 {status}\r\nContent-Length: {length}\r\nETag: {tag}\r\n{extra}\r\n")
        };
        let response: (String, &[u8]) = match mode {
            Mode::Missing => (head("404 Not Found", "", 0), &[]),
            Mode::RedirectToHttp => (
                head(
                    "302 Found",
                    &format!("Location: http://127.0.0.1:{port}/elsewhere\r\n"),
                    0,
                ),
                &[],
            ),
            Mode::Whole => (head("200 OK", "", data.len()), data),
            Mode::Ranges => {
                if if_match.as_deref().is_some_and(|m| m != tag) {
                    (head("412 Precondition Failed", "", 0), &[])
                } else {
                    let spec = range.unwrap();
                    let (a, b) = spec
                        .strip_prefix("bytes=")
                        .unwrap()
                        .split_once('-')
                        .unwrap();
                    // A suffix range (`bytes=-N`) is the last N bytes.
                    let (a, b) = if a.is_empty() {
                        let n: usize = b.parse().unwrap();
                        (data.len().saturating_sub(n), data.len() - 1)
                    } else {
                        (
                            a.parse().unwrap(),
                            b.parse::<usize>().unwrap().min(data.len() - 1),
                        )
                    };
                    let body = &data[a..=b];
                    (
                        head(
                            "206 Partial Content",
                            &format!("Content-Range: bytes {a}-{b}/{}\r\n", data.len()),
                            body.len(),
                        ),
                        body,
                    )
                }
            }
        };
        if out.write_all(response.0.as_bytes()).is_err() || out.write_all(response.1).is_err() {
            return;
        }
    }
}

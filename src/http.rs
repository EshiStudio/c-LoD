//! Embedded HTTP server: serves the generated resource pack to vanilla
//! clients. std-only, one accept thread, one thread per connection (packs
//! are large but client downloads are rare).

use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::{Arc, RwLock};

pub struct PackServer {
    pub bytes: RwLock<Arc<Vec<u8>>>,
}

impl PackServer {
    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            bytes: RwLock::new(Arc::new(Vec::new())),
        })
    }

    pub fn set_pack(&self, zip: Vec<u8>) {
        if let Ok(mut b) = self.bytes.write() {
            *b = Arc::new(zip);
        }
    }

    pub fn serve(self: &Arc<Self>, port: u16) -> std::io::Result<()> {
        let listener = TcpListener::bind(("0.0.0.0", port))?;
        crate::log_info(&format!(
            "c-LoD pack server listening on 0.0.0.0:{port} (GET /clod-pack.zip)"
        ));
        for stream in listener.incoming() {
            match stream {
                Ok(s) => {
                    let srv = Arc::clone(self);
                    std::thread::spawn(move || {
                        let _ = handle(s, &srv);
                    });
                }
                Err(_) => continue,
            }
        }
        Ok(())
    }
}

fn handle(mut stream: TcpStream, srv: &PackServer) -> std::io::Result<()> {
    let mut buf = [0u8; 4096];
    let mut req = Vec::new();
    loop {
        let n = stream.read(&mut buf)?;
        if n == 0 {
            break;
        }
        req.extend_from_slice(&buf[..n]);
        if req.windows(4).any(|w| w == b"\r\n\r\n") || req.len() > 16 * 1024 {
            break;
        }
    }
    let req = String::from_utf8_lossy(&req);
    let first_line = req.lines().next().unwrap_or("");
    let mut parts = first_line.split_whitespace();
    let method = parts.next().unwrap_or("");
    let path = parts.next().unwrap_or("");

    let pack = srv.bytes.read().ok().map(|b| Arc::clone(&b));
    let Some(pack) = pack else {
        write_status(&mut stream, 503, "service unavailable", b"pack not ready")?;
        return Ok(());
    };

    let head_only = method == "HEAD";
    if (method == "GET" || head_only) && (path == "/clod-pack.zip" || path == "/clod-pack.zip?") {
        stream.write_all(b"HTTP/1.1 200 OK\r\n")?;
        stream.write_all(b"Content-Type: application/zip\r\n")?;
        stream.write_all(format!("Content-Length: {}\r\n", pack.len()).as_bytes())?;
        stream.write_all(b"Cache-Control: no-cache\r\n")?;
        stream.write_all(b"Connection: close\r\n\r\n")?;
        if !head_only {
            stream.write_all(&pack)?;
        }
        stream.flush()?;
        return Ok(());
    }

    write_status(&mut stream, 404, "not found", b"c-LoD: use /clod-pack.zip")
}

fn write_status(stream: &mut TcpStream, code: u16, msg: &str, body: &[u8]) -> std::io::Result<()> {
    let resp = format!(
        "HTTP/1.1 {code} {msg}\r\nContent-Type: text/plain\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        body.len()
    );
    stream.write_all(resp.as_bytes())?;
    stream.write_all(body)?;
    stream.flush()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::TcpStream as CStream;
    use std::time::Duration;

    #[test]
    fn serves_pack() {
        let srv = PackServer::new();
        srv.set_pack(vec![1, 2, 3, 4, 5, 6, 7, 8]);
        let srv2 = Arc::clone(&srv);
        let h = std::thread::spawn(move || {
            let _ = srv2.serve(18399);
        });
        std::thread::sleep(Duration::from_millis(300));

        let mut c = CStream::connect(("127.0.0.1", 18399)).expect("connect");
        c.set_read_timeout(Some(Duration::from_secs(3))).unwrap();
        c.write_all(b"GET /clod-pack.zip HTTP/1.1\r\nHost: t\r\n\r\n")
            .unwrap();
        let mut resp = Vec::new();
        let _ = c.read_to_end(&mut resp);
        let text = String::from_utf8_lossy(&resp);
        assert!(text.starts_with("HTTP/1.1 200 OK"));
        assert!(text.contains("Content-Length: 8"));
        assert!(resp.ends_with(&[1, 2, 3, 4, 5, 6, 7, 8]));

        // 404 path
        let mut c2 = CStream::connect(("127.0.0.1", 18399)).unwrap();
        c2.set_read_timeout(Some(Duration::from_secs(3))).unwrap();
        c2.write_all(b"GET /nope HTTP/1.1\r\nHost: t\r\n\r\n")
            .unwrap();
        let mut r2 = Vec::new();
        let _ = c2.read_to_end(&mut r2);
        assert!(String::from_utf8_lossy(&r2).starts_with("HTTP/1.1 404"));

        drop(h);
    }
}

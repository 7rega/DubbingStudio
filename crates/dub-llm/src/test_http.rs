//! Поддельный HTTP-сервер для тестов провайдеров: отвечает заготовленными ответами по порядку и запоминает
//! сырые запросы. Каждое соединение — один запрос (`Connection: close`).

use std::io::{Read, Write};
use std::net::TcpListener;
use std::sync::{Arc, Mutex};

pub struct Reply {
    status: u16,
    content_type: String,
    body: Vec<u8>,
}

impl Reply {
    pub fn json(status: u16, body: &str) -> Self {
        Reply { status, content_type: "application/json".into(), body: body.as_bytes().to_vec() }
    }

    pub fn bytes(status: u16, content_type: &str, body: Vec<u8>) -> Self {
        Reply { status, content_type: content_type.into(), body }
    }
}

pub struct FakeServer {
    port: u16,
    requests: Arc<Mutex<Vec<String>>>,
}

impl FakeServer {
    /// `http://127.0.0.1:<port>` — без хвоста.
    pub fn base(&self) -> String {
        format!("http://127.0.0.1:{}", self.port)
    }

    /// Сырой i-й запрос (заголовки + тело); ждёт, пока он придёт.
    pub fn request(&self, index: usize) -> String {
        for _ in 0..500 {
            if let Some(request) = self.requests.lock().unwrap().get(index) {
                return request.clone();
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        panic!("request {index} never arrived");
    }

    pub fn count(&self) -> usize {
        self.requests.lock().unwrap().len()
    }
}

/// Поднять сервер на свободном порту с ответами `replies` по порядку.
pub fn serve(replies: Vec<Reply>) -> FakeServer {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind fake server");
    let port = listener.local_addr().expect("fake server address").port();
    let requests = Arc::new(Mutex::new(Vec::new()));
    let seen = requests.clone();
    std::thread::spawn(move || {
        for reply in replies {
            let Ok((mut stream, _)) = listener.accept() else { return };
            let raw = read_request(&mut stream);
            seen.lock().unwrap().push(raw);
            let reason = if reply.status < 400 { "OK" } else { "ERR" };
            let head = format!(
                "HTTP/1.1 {} {reason}\r\nContent-Type: {}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                reply.status,
                reply.content_type,
                reply.body.len()
            );
            let _ = stream.write_all(head.as_bytes());
            let _ = stream.write_all(&reply.body);
            let _ = stream.flush();
        }
    });
    FakeServer { port, requests }
}

fn read_request(stream: &mut std::net::TcpStream) -> String {
    let mut data = Vec::new();
    let mut buffer = [0u8; 65536];
    while let Ok(read) = stream.read(&mut buffer) {
        if read == 0 {
            break;
        }
        data.extend_from_slice(&buffer[..read]);
        let text = String::from_utf8_lossy(&data);
        if let Some(end) = text.find("\r\n\r\n") {
            let length = text[..end]
                .lines()
                .find_map(|line| {
                    let (name, value) = line.split_once(':')?;
                    name.trim().eq_ignore_ascii_case("content-length").then(|| value.trim().parse::<usize>().ok()).flatten()
                })
                .unwrap_or(0);
            if data.len() >= end + 4 + length {
                break;
            }
        }
    }
    String::from_utf8_lossy(&data).into_owned()
}

/// JSON-тело сырого запроса.
pub fn body_json(request: &str) -> serde_json::Value {
    let start = request.find("\r\n\r\n").expect("request has a body") + 4;
    serde_json::from_str(&request[start..]).expect("request body is JSON")
}

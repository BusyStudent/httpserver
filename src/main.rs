use std::collections::HashMap;
use std::io::{BufRead, BufReader, ErrorKind, Write};
use std::net::TcpListener;
use std::net::TcpStream;
use std::str::{self, Chars};

// Parse the given string and return the method and path
fn parse_request_line(line: &str) -> Option<(&str, &str)> {
    let mut s = line.split(" ");
    let method = s.next()?;
    let path = s.next()?;
    let _ver = s.next()?;
    if !s.next().is_none() { // It should just 3 items
        return None;
    }
    return Some((method, path));
}

fn status_code_to_string(code: i32) -> &'static str {
    match code {
        200 => "OK",
        201 => "Created",
        202 => "Accepted",
        204 => "No Content",
        301 => "Moved Permanently",
        302 => "Found",
        304 => "Not Modified",
        400 => "Bad Request",
        401 => "Unauthorized",
        403 => "Forbidden",
        404 => "Not Found",
        405 => "Method Not Allowed",
        500 => "Internal Server Error",
        501 => "Not Implemented",
        502 => "Bad Gateway",
        503 => "Service Unavailable",
        504 => "Gateway Timeout",
        _ => panic!("WTF?")
    }
}

fn write_reply(stream: &mut TcpStream, code: i32, content: &[u8]) -> Result<(), std::io::Error> {
    let reply = format!(
        "HTTP/1.1 {} {}\r\nContent-Length: {}\r\n\r\n",
        code,
        status_code_to_string(code),
        content.len()
    );
    stream.write_all(reply.as_bytes())?;
    stream.write_all(content)?;
    stream.flush()?;
    Ok(())
}

fn write_bad_reply(stream: &mut TcpStream) -> Result<(), std::io::Error> {
    write_reply(stream, 500, "<html>bad requests</html>".as_bytes())?;
    Ok(())
}

fn gen_fs_page(path: &str) -> Result<Vec::<u8>, std::io::Error> {
    // Dispatch path by query
    if std::fs::metadata(path)?.is_dir() {
        let mut content = String::new();
        content.push_str("<html><meta charset=\"utf-8\" /><body><ul>");
        for dir in std::fs::read_dir(path)? {
            let name = dir?.file_name().to_string_lossy().into_owned();
            let mut pathname = String::from(path);
            if !pathname.ends_with("/") {
                pathname.push('/');
            }
            pathname.push_str(&encode_url(name.as_str()));
            content.push_str(&format!("<li><a href=\"{}\">{}</a></li>", pathname, name));
        }
        content.push_str("</ul></body></html>");
        return Ok(content.into_bytes());
    }
    else {
        return Ok(std::fs::read(path)?);
    }
}

fn decode_url(s: &str) -> Option<String> {
    let mut out = String::new();
    let mut chars = s.chars();
    let read = |chars: &mut Chars<'_> | -> Option<u8> {
        let h1 = chars.next()?;
        let h2 = chars.next()?;
        let hex = format!("{h1}{h2}");
        return Some(u8::from_str_radix(hex.as_str(), 16).ok()?);
    };
    while let Some(c) = chars.next() {
        if c == '%' { // Got Utf8 code point here
            let byte = read(&mut chars)?;
            if byte < 127 {
                out.push(char::from_u32(byte as u32)?);
                continue;
            }
            let mut codepoints = Vec::<u8>::new();
            codepoints.push(byte);
            loop {
                match str::from_utf8(codepoints.as_slice()) {
                    Ok(s) => {
                        out.push_str(s);
                        break;
                    },
                    Err(_) => {
                        // Collect the next codepoint
                        let next = chars.next()?;
                        if next != '%' {
                            // Utf8 sequence end !!!
                            return None;
                        }
                        codepoints.push(read(&mut chars)?);
                    }
                }
            }
        }
        else {
            out.push(c);
        }
    }

    Some(out)
}

fn encode_url(s: &str) -> String {
    let mut out = String::new();

    for ch in s.chars() {
        if ch.is_ascii() {
            if ch.is_ascii_alphabetic() || ch.is_ascii_digit() ||  ch == '-' || ch == '_' || ch == '.' || ch == '~' {
                // Is Part of char can directly sent
                out.push(ch);
                continue;
            }
        }
        // We need to encode it
        let mut buffer = [0u8; 4];
        for uchar in ch.encode_utf8(&mut buffer).as_bytes() {
            out.push('%');
            out.push_str(&format!("{uchar:X}"));
        }
    }

    return out;
}

fn handle_client(mut stream: TcpStream) -> Result<(), std::io::Error> {
    // First Get the first line
    let peeraddr = stream.peer_addr()?;
    println!("handling peer {peeraddr}");

    let reader_stream = stream.try_clone()?;
    let mut reader = BufReader::new(reader_stream);

    loop { // For Handle each per requests
        let mut buffer = String::new();

        // Read All Http Headers
        if reader.read_line(&mut buffer)? == 0 { // EOF
            println!("EOF, Quiting...");
            return Ok(());
        }
        let (method, path) = match parse_request_line(buffer.trim()) {
            Some(some) => some,
            None => return Ok(()),
        };
        let path = match decode_url(path) {
            Some(what) => what,
            None => return Ok(()),
        };
        println!("method {method} path {path}");

        // Read all headers
        let mut headers = HashMap::new();
        let mut line = String::new();
        loop {
            if reader.read_line(&mut line)? == 0 {
                return Ok(());
            }
            let myline = line.trim();
            if myline.len() == 0 { // The last \r\n
                break;
            }
            // Split it by ': '
            let kvs : Vec<&str> = myline.split(": ").collect();
            if kvs.len() != 2 {
                println!("parse the headers failed, expected 2, got {}", kvs.len());
                write_bad_reply(&mut stream)?;
                return Ok(());
            }
            headers.insert(String::from(kvs[0].trim()), String::from(kvs[1].trim()));
            line.clear();
        }
        println!("headers: {:?}", headers);

        // Dispatch path by query
        match gen_fs_page(path.as_str()) {
            Ok(content) => write_reply(&mut stream, 200, content.as_slice())?,
            Err(err) => match err.kind() {
                ErrorKind::NotFound => write_reply(&mut stream, 404, "<html>404</html>".as_bytes())?,
                _ => write_reply(&mut stream, 500, "<html>500</html>".as_bytes())?
            }
        }
    }
}

fn main() {
    let listener = match TcpListener::bind("127.0.0.1:25565") {
        Ok(what) => what,
        Err(err) => {
            println!("failed to create a tcp listener by {err}");
            return;
        }
    };
    println!("Listen on {}", listener.local_addr().expect("it should never fail"));
    loop {
        let (stream, addr) = match listener.accept() {
            Ok(what) => what,
            Err(err) => {
                println!("failed to accept tcp listener {err}");
                return;
            }
        };
        println!("incoming client from {addr}");
        std::thread::spawn(|| {
            if let Err(e) = handle_client(stream) {
                println!("Error handling client: {}", e);
            }
        });
    }
}

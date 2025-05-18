use std::collections::HashMap;
use std::io;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader, ErrorKind};
use tokio::net::TcpListener;
use tokio::net::TcpStream;
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

async fn write_reply(stream:  &mut (impl AsyncWriteExt + Unpin), code: i32, content: &[u8]) -> io::Result<()> {
    let reply = format!(
        "HTTP/1.1 {} {}\r\nContent-Length: {}\r\n\r\n",
        code,
        status_code_to_string(code),
        content.len()
    );
    stream.write_all(reply.as_bytes()).await?;
    stream.write_all(content).await?;
    stream.flush().await?;
    Ok(())
}

async fn write_bad_reply(stream: &mut (impl AsyncWriteExt + Unpin)) -> io::Result<()> {
    write_reply(stream, 500, "<html>bad requests</html>".as_bytes()).await?;
    Ok(())
}

async fn gen_fs_page(path: &str) -> io::Result<Vec::<u8> > {
    // Dispatch path by query
    if tokio::fs::metadata(path).await?.is_dir() {
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
        return Ok(tokio::fs::read(path).await?);
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

async fn handle_client(mut stream: TcpStream) -> io::Result<()> {
    // First Get the first line
    let peeraddr = stream.peer_addr()?;
    println!("handling peer {peeraddr}");

    let (reader, mut writer) = stream.split();
    let mut reader = BufReader::new(reader);

    loop { // For Handle each per requests
        let mut buffer = String::new();

        // Read All Http Headers
        if reader.read_line(&mut buffer).await? == 0 { // EOF
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
            if reader.read_line(&mut line).await? == 0 {
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
                write_bad_reply(&mut writer).await?;
                return Ok(());
            }
            headers.insert(String::from(kvs[0].trim()), String::from(kvs[1].trim()));
            line.clear();
        }
        println!("headers: {:?}", headers);

        // Dispatch path by query
        match gen_fs_page(path.as_str()).await {
            Ok(content) => write_reply(&mut writer, 200, content.as_slice()).await?,
            Err(err) => match err.kind() {
                ErrorKind::NotFound => write_reply(&mut writer, 404, "<html>404</html>".as_bytes()).await?,
                _ => write_reply(&mut writer, 500, "<html>500</html>".as_bytes()).await?
            }
        }
    }
}

#[tokio::main]
async fn main() {
    let listener = match TcpListener::bind("127.0.0.1:25565").await {
        Ok(what) => what,
        Err(err) => {
            println!("failed to create a tcp listener by {err}");
            return;
        }
    };
    println!("Listen on {}", listener.local_addr().expect("it should never fail"));
    loop {
        let (stream, addr) = match listener.accept().await {
            Ok(what) => what,
            Err(err) => {
                println!("failed to accept tcp listener {err}");
                return;
            }
        };
        println!("incoming client from {addr}");
        tokio::task::spawn(async move {
            if let Err(e) = handle_client(stream).await {
                println!("Error handling client: {}", e);
            }
        });
    }
}

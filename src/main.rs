use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use axum::{
    body::Body,
    http::{header, Request, StatusCode},
    response::{IntoResponse, Response},
    Router,
};
use clap::Parser;
use percent_encoding::percent_decode_str;
use tokio::net::TcpListener;

/// http 静态 mock 服务器.
#[derive(Parser)]
#[command(name = "http-mock-server", version, about)]
struct Cli {
    /// 监听端口
    #[arg(short, long, default_value_t = 8380, value_name = "PORT")]
    port: u16,

    /// 监听 IP
    #[arg(short = 'i', long, default_value = "127.0.0.1", value_name = "IP")]
    ip: String,

    /// 静态文件目录, 可多次指定, 从前到后按优先级搜索
    #[arg(short, long, value_name = "DIR")]
    dirs: Vec<PathBuf>,
}

#[tokio::main]
async fn main() {
    let cli = Cli::parse();

    let mut dirs = cli.dirs;
    if dirs.is_empty() {
        dirs.push(PathBuf::from("."));
    }

    let app = Router::new().fallback({
        let dirs = Arc::new(dirs);
        move |req: Request<Body>| {
            let dirs = Arc::clone(&dirs);
            async move { serve(req, dirs).await }
        }
    });

    let addr: SocketAddr = format!("{}:{}", cli.ip, cli.port).parse().expect("无效的 IP:端口");
    let listener = TcpListener::bind(addr).await.expect("绑定监听地址失败");
    println!("http-mock-server 已启动: http://{addr}");

    if let Err(e) = axum::serve(listener, app).await {
        eprintln!("服务器运行出错: {e}");
    }
}

async fn serve(request: Request<Body>, dirs: Arc<Vec<PathBuf>>) -> Response {
    // 先做百分号解码, 再规范化路径.
    // 解码必须在 normalize 之前: 既让 %E4%B8%AD 之类的编码文件名能命中,
    // 又保证解码出的 `..` (%2e%2e) 仍会被 normalize 拦截, 不产生穿越.
    let raw_path = request.uri().path();
    let decoded = percent_decode_str(raw_path).decode_utf8_lossy();

    let Some(rel) = normalize(&decoded) else {
        return StatusCode::NOT_FOUND.into_response();
    };

    match resolve(&dirs, &rel).await {
        Some((path, content_type)) => {
            println!("请求 {:?} -> {}", rel, path.display());
            match tokio::fs::read(&path).await {
                Ok(bytes) => (
                    [(header::CONTENT_TYPE, content_type)],
                    bytes,
                )
                    .into_response(),
                Err(_) => StatusCode::INTERNAL_SERVER_ERROR.into_response(),
            }
        }
        None => StatusCode::NOT_FOUND.into_response(),
    }
}

/// 按目录优先级查找请求路径对应的文件, 并在需要时回退 `.json` 后缀.
async fn resolve(dirs: &[PathBuf], rel: &str) -> Option<(PathBuf, String)> {
    // 优先搜索原路径
    for dir in dirs {
        let p = dir.join(rel);
        if is_file(&p).await {
            let ct = content_type(&p);
            return Some((p, ct));
        }
        if is_dir(&p).await {
            // 请求路径指向目录: 先 index.html, 再 index.json
            for index in ["index.html", "index.json"] {
                let ip = p.join(index);
                if is_file(&ip).await {
                    let ct = content_type(&ip);
                    return Some((ip, ct));
                }
            }
        }
    }

    // 原路径不存在, 追加 `.json` 后缀再按优先级搜索
    let json_rel = format!("{rel}.json");
    for dir in dirs {
        let p = dir.join(&json_rel);
        if is_file(&p).await {
            let ct = content_type(&p);
            return Some((p, ct));
        }
    }

    None
}

/// 去掉 URI 前导 `/`, 并规范化为安全相对路径(阻止目录穿越); 非法(尝试逃出)时返回 None.
fn normalize(path: &str) -> Option<String> {
    let mut stack: Vec<&str> = Vec::new();
    for comp in path.split('/') {
        match comp {
            "" | "." => {}
            ".." => {
                if stack.pop().is_none() {
                    return None;
                }
            }
            c => stack.push(c),
        }
    }
    Some(stack.join("/"))
}

async fn is_file(p: &Path) -> bool {
    tokio::fs::metadata(p).await.map(|m| m.is_file()).unwrap_or(false)
}

async fn is_dir(p: &Path) -> bool {
    tokio::fs::metadata(p).await.map(|m| m.is_dir()).unwrap_or(false)
}

/// 根据文件扩展名返回 Content-Type.
fn content_type(p: &Path) -> String {
    match p
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_ascii_lowercase()
        .as_str()
    {
        "json" => "application/json",
        "html" | "htm" => "text/html",
        "css" => "text/css",
        "js" | "mjs" => "text/javascript",
        "txt" | "md" => "text/plain",
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "svg" => "image/svg+xml",
        "webp" => "image/webp",
        "ico" => "image/x-icon",
        "wasm" => "application/wasm",
        "xml" => "application/xml",
        "pdf" => "application/pdf",
        _ => "application/octet-stream",
    }
    .to_string()
}
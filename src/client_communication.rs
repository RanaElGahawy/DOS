use hyper::{Client, Request, Body, Method};
use hyper_rustls::HttpsConnector;
use std::sync::Arc;
use tokio::net::{TcpListener, TcpStream};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use serde_json::json;
use std::error::Error;

type SheetsClient = Arc<Client<HttpsConnector<hyper::client::HttpConnector>, Body>>;

pub async fn listen_for_requests(
    client_socket: Arc<TcpListener>,
    sheets_client: SheetsClient,
    access_token: String,
) -> Result<(), Box<dyn Error>> {
    println!("Server running on port 8081...");

    loop {
        let (socket, addr) = client_socket.accept().await?;
        let sheets_client_clone = Arc::clone(&sheets_client);
        let token_clone = access_token.clone();

        tokio::spawn(async move {
            if let Err(e) = handle_client(socket, addr, sheets_client_clone, token_clone).await {
                eprintln!("Error handling client {}: {}", addr, e);
            }
        });
    }
}

async fn handle_client(
    mut socket: TcpStream,
    addr: std::net::SocketAddr,
    sheets_client: SheetsClient,
    access_token: String,
) -> Result<(), Box<dyn Error>> {
    let mut buf = vec![0u8; 1024];
    let n = socket.read(&mut buf).await?;
    if n == 0 {
        return Ok(());
    }

    let request = String::from_utf8_lossy(&buf[..n]);
    match request.as_ref() {
        "JOIN" => handle_join_request(addr, sheets_client, access_token, &mut socket).await,
        _ => Ok(()),
    }
}

async fn handle_join_request(
    addr: std::net::SocketAddr,
    sheets_client: SheetsClient,
    access_token: String,
    socket: &mut TcpStream,
) -> Result<(), Box<dyn Error>> {
    let client_id = format!("client_{}", addr.port());
    let ip_port = addr.to_string();

    add_client_to_dos(&sheets_client, access_token, client_id.clone(), ip_port.clone()).await?;
    socket.write_all(client_id.as_bytes()).await?;
    println!("Client joined: {}", client_id);

    Ok(())
}

async fn add_client_to_dos(
    sheets_client: &SheetsClient,
    access_token: String,
    client_id: String,
    ip_port: String,
) -> Result<(), Box<dyn Error>> {
    let spreadsheet_id = "12SqSHonSlPVo8JXcj2Or1cmXOlEPpIjxQ64yZLOHZIg";
    let range = "Sheet1!A:B";

    // Prepare the data to append
    let values = json!({
        "values": [[client_id, ip_port, "ACTIVE".to_string()]]
    });

    // Build the request URL
    let url = format!(
        "https://sheets.googleapis.com/v4/spreadsheets/{}/values/{}:append?valueInputOption=RAW",
        spreadsheet_id, range
    );

    // Create the HTTP request
    let req = Request::builder()
        .method(Method::POST)
        .uri(url)
        .header("Authorization", format!("Bearer {}", access_token))
        .header("Content-Type", "application/json")
        .body(Body::from(values.to_string()))?;

    // Send the request
    let res = sheets_client.request(req).await?;
    let status = res.status();

    if status.is_success() {
        println!("Successfully appended client data to Google Sheets");
        Ok(())
    } else {
        let body_bytes = hyper::body::to_bytes(res.into_body()).await?;
        let body_str = String::from_utf8_lossy(&body_bytes);
        eprintln!("Failed to append to Google Sheets: {}: {}", status, body_str);
        Err(Box::new(std::io::Error::new(std::io::ErrorKind::Other, "Failed to append data")))
    }
}

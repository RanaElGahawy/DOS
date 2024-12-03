use crate::encryption;
use crate::dos_handling;
use hyper::{Body, Client};
use hyper_rustls::HttpsConnector;
use tokio::sync::Mutex;
use std::sync::Arc;
use tokio::net::{TcpListener, TcpStream};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use std::error::Error;

type SheetsClient = Arc<Client<HttpsConnector<hyper::client::HttpConnector>, Body>>;

pub async fn listen_for_requests(
    client_socket: Arc<TcpListener>,
    sheets_client: SheetsClient,
    access_token: String,
    request_count: Arc<Mutex<u32>>,  // Shared request count
    is_leader: Arc<Mutex<bool>>,    // Shared is_leader flag
) -> Result<(), Box<dyn Error>> {
    println!("Server running on port 8081...");

    loop {
        // Check if the server is the leader
        let is_leader_status = {
            let leader_guard = is_leader.lock().await; // Lock the is_leader flag
            *leader_guard
        };

        if !is_leader_status {
            println!("Server is not the leader. Waiting...");
            tokio::time::sleep(tokio::time::Duration::from_secs(1)).await; // Wait and retry
            continue;
        }

        // Accept incoming client connections
        let (socket, addr) = client_socket.accept().await?;
        let sheets_client_clone = Arc::clone(&sheets_client);
        let token_clone = access_token.clone();
        let request_count_clone = Arc::clone(&request_count);

        // Spawn a task to handle the client
        tokio::spawn(async move {
            if let Err(e) = handle_client(socket, addr, sheets_client_clone, token_clone, request_count_clone).await {
                eprintln!("Error handling client {}: {}", addr, e);
            }
        });
    }
}

async fn handle_client(
    socket: TcpStream,
    addr: std::net::SocketAddr,
    sheets_client: SheetsClient,
    access_token: String,
    request_count: Arc<Mutex<u32>>, // Shared request count
) -> Result<(), Box<dyn Error>> {
    let socket = Arc::new(Mutex::new(socket)); // Wrap socket in Arc<Mutex> for shared access
    let mut buf = vec![0u8; 1024];

    let n = socket.lock().await.read(&mut buf).await?;
    if n == 0 {
        return Ok(());
    }

    let request = String::from_utf8_lossy(&buf[..n]).to_string();
    let parts: Vec<String> = request.split_whitespace().map(|s| s.to_string()).collect();
    println!("Received request: {}", request);

    match parts.get(0) {
        Some(cmd) if cmd == "JOIN" => {
            let sheets_client = Arc::clone(&sheets_client);
            let access_token = access_token.clone();
            let socket = Arc::clone(&socket);
            tokio::spawn(async move {
                if let Err(e) =
                    dos_handling::handle_join_request(addr, sheets_client, access_token, &mut *socket.lock().await)
                        .await
                {
                    eprintln!("Failed to handle JOIN request: {}", e);
                }
            });
        }
        Some(cmd) if cmd == "REJOIN" && parts.len() > 1 => {
            let client_id = parts[1].clone();
            let sheets_client = Arc::clone(&sheets_client);
            let access_token = access_token.clone();
            let socket = Arc::clone(&socket);
            tokio::spawn(async move {
                if let Err(e) = dos_handling::handle_rejoin_request(
                    &client_id,
                    addr,
                    sheets_client,
                    access_token,
                    &mut *socket.lock().await,
                )
                .await
                {
                    eprintln!("Failed to handle REJOIN request: {}", e);
                }
            });
        }
        Some(cmd) if cmd == "SIGN_OUT" && parts.len() > 1 => {
            let client_id = parts[1].clone();
            let sheets_client = Arc::clone(&sheets_client);
            let access_token = access_token.clone();
            let socket = Arc::clone(&socket);
            tokio::spawn(async move {
                if let Err(e) = dos_handling::handle_sign_out_request(
                    &client_id,
                    sheets_client,
                    access_token,
                    &mut *socket.lock().await,
                )
                .await
                {
                    eprintln!("Failed to handle SIGN_OUT request: {}", e);
                }
            });
        }
        Some(cmd) if cmd == "SHOW_ACTIVE_CLIENTS" => {
            let sheets_client = Arc::clone(&sheets_client);
            let access_token = access_token.clone();
            let socket = Arc::clone(&socket);
            tokio::spawn(async move {
                if let Err(e) = dos_handling::handle_show_active_clients_request(
                    addr,
                    sheets_client,
                    access_token,
                    &mut *socket.lock().await,
                )
                .await
                {
                    eprintln!("Failed to handle SHOW_ACTIVE_CLIENTS request: {}", e);
                }
            });
        }
        Some(cmd) if cmd == "UNREACHABLE" && parts.len() > 1 => {
            let client_id = parts[1].clone();
            let sheets_client = Arc::clone(&sheets_client);
            let access_token = access_token.clone();
            tokio::spawn(async move {
                if let Err(e) = dos_handling::handle_unreachable_id(&client_id, sheets_client, access_token).await {
                    eprintln!("Failed to handle UNREACHABLE request: {}", e);
                }
            });
        }
        Some(cmd) if cmd == "ENCRYPTION" => {
            let request_count = Arc::clone(&request_count);
            let socket = Arc::clone(&socket);
            tokio::spawn(async move {
                if let Err(e) = handle_encryption(socket, addr, request_count).await {
                    eprintln!("Failed to handle ENCRYPTION request: {}", e);
                }
            });
        }
        _ => {
            eprintln!("Invalid request received: {}", request);
        }
    }

    Ok(())
}


async fn handle_encryption(
    socket: Arc<Mutex<TcpStream>>,
    addr: std::net::SocketAddr,
    request_count: Arc<Mutex<u32>>,
) -> Result<(), Box<dyn Error>> {
    // Acknowledge the ENCRYPTION command
    {
        let mut socket = socket.lock().await;
        let ack_response = "ACK";
        socket.write_all(ack_response.as_bytes()).await?;
        socket.flush().await?;
        println!("Acknowledgment sent for ENCRYPTION command.");
    }

    // Step 1: Read the length prefix (4 bytes)
    let mut len_buf = [0u8; 4];
    {
        let mut socket = socket.lock().await;
        socket.read_exact(&mut len_buf).await?;
    }
    let data_length = u32::from_be_bytes(len_buf) as usize;
    println!("Expecting {} bytes of image data.", data_length);

    // Step 2: Read the image data
    let mut image_data = vec![0u8; data_length];
    {
        let mut socket = socket.lock().await;
        socket.read_exact(&mut image_data).await?;
    }
    println!("Received {} bytes of image data.", image_data.len());

    // Save the received image
    let received_image_path = format!("received_{}.png", addr.port());
    let encoded_file_name = format!("encoded_{}.png", addr.port());
    tokio::fs::write(&received_image_path, &image_data).await?;
    println!("Received image and saved as: {}", received_image_path);

    // Increment request count
    {
        let mut count = request_count.lock().await;
        *count += 1;
        println!("Request count incremented to: {}", *count);
    }

    // Process and send back the encoded image
    encryption::encode_and_send(
        received_image_path,
        encoded_file_name,
        socket, // Pass the Arc<Mutex<TcpStream>> directly
        Arc::clone(&request_count),
    )
    .await?;

    println!("Response sent to client.");
    Ok(())
}

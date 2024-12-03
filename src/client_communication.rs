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

// Function to listen for incoming requests from clients
pub async fn listen_for_requests(
    client_socket: Arc<TcpListener>,
    sheets_client: SheetsClient,
    access_token: String,
    request_count: Arc<Mutex<u32>>,  // Shared request count
) -> Result<(), Box<dyn Error>> {
    println!("Server running on port 8081...");

    // Create a channel for passing encryption tasks to workers

    loop {
        let (socket, addr) = client_socket.accept().await?;
        let sheets_client_clone = Arc::clone(&sheets_client);
        let token_clone = access_token.clone();
        let request_count_clone = Arc::clone(&request_count);  // Clone the request_count Arc for each client

        tokio::spawn(async move {
            if let Err(e) = handle_client(socket, addr, sheets_client_clone, token_clone, request_count_clone).await {
                eprintln!("Error handling client {}: {}", addr, e);
            }
        });
    }
}

// Client request handler that processes each incoming command
async fn handle_client(
    mut socket: TcpStream,
    addr: std::net::SocketAddr,
    sheets_client: SheetsClient,
    access_token: String,
    request_count: Arc<Mutex<u32>>,  // Shared request count
) -> Result<(), Box<dyn Error>> {
    let mut buf = vec![0u8; 1024];
    let n = socket.read(&mut buf).await?;
    if n == 0 {
        return Ok(());
    }

    let request = String::from_utf8_lossy(&buf[..n]).to_string();  // Create full String here
    let parts: Vec<String> = request.split_whitespace().map(|s| s.to_string()).collect();  // Convert to owned Vec<String>
    println!("Received request: {}", request);

    match parts.get(0) {
        Some(cmd) if cmd == "JOIN" => {
            dos_handling::handle_join_request(addr, sheets_client, access_token, &mut socket).await
        }
        Some(cmd) if cmd == "REJOIN" && parts.len() > 1 => {
            let client_id = &parts[1];
            dos_handling::handle_rejoin_request(client_id, addr, sheets_client, access_token, &mut socket).await
        }
        Some(cmd) if cmd == "SIGN_OUT" && parts.len() > 1 => {
            let client_id = &parts[1];
            dos_handling::handle_sign_out_request(client_id, sheets_client, access_token, &mut socket).await
        }
        Some(cmd) if cmd == "SHOW_ACTIVE_CLIENTS" => {
            dos_handling::handle_show_active_clients_request(addr, sheets_client, access_token, &mut socket).await
        }
        Some(cmd) if cmd == "UNREACHABLE" && parts.len() > 1 => {
            let client_id = &parts[1];
            dos_handling::handle_unreachable_id(client_id, sheets_client, access_token).await
        }
        Some(cmd) if cmd == "ENCRYPTION" => {
            // Acknowledge the ENCRYPTION command
            let ack_response = "ACK";
            if let Err(e) = socket.write_all(ack_response.as_bytes()).await {
                eprintln!("Failed to send acknowledgment: {}", e);
                return Ok(());
            }
            socket.flush().await?;
            println!("Acknowledgment sent for ENCRYPTION command.");
        
            // Step 1: Read the length prefix (4 bytes)
            let mut len_buf = [0u8; 4];
            socket.read_exact(&mut len_buf).await?;
            let data_length = u32::from_be_bytes(len_buf) as usize;
            println!("Expecting {} bytes of image data.", data_length);
        
            // Step 2: Read the image data based on the length prefix
            let mut image_data = vec![0u8; data_length];
            socket.read_exact(&mut image_data).await?;
            println!("Received {} bytes of image data.", image_data.len());
        
            // Step 3: Save the received image (for demonstration)
            let received_image_path = "received_image.png";
            let encoded_file_name = "encoded.png";
            let mut file = tokio::fs::File::create(received_image_path).await?;
            file.write_all(&image_data).await?;
            println!("Received image and saved as: {}", received_image_path);
            
            // Increment request count (if needed)
            {
                let mut count = request_count.lock().await;
                *count += 1;
                println!("Request count incremented to: {}", *count);
            }

            // Process and send back the encoded image
            encryption::encode_and_send(
                received_image_path.to_string(),
                encoded_file_name.to_string(),
                &mut socket,
                request_count.clone(),
            )
            .await?;
        
            println!("Response sent to client.");
        
            Ok(())
        }
        
        _ => {
            eprintln!("Invalid request received: {}", request);
            Ok(())
        }
    }
}
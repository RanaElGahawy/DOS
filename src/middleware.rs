use hyper::{Client, Body};
use hyper_rustls::{HttpsConnector, HttpsConnectorBuilder};
use yup_oauth2::{read_service_account_key, ServiceAccountAuthenticator};
use std::sync::Arc;
use tokio::net::{TcpListener, UdpSocket};
use std::net::SocketAddr;
use std::error::Error;
use tokio::sync::Mutex;

mod client_communication;
mod encryption;
mod dos_handling;
mod election;

pub use encryption::*;
pub use dos_handling::*;


#[tokio::main(flavor = "multi_thread", worker_threads = 2)]
async fn main() -> Result<(), Box<dyn Error>> {
    let args: Vec<String> = std::env::args().collect();
    if args.len() < 4 {
        eprintln!("Usage: {} <self_ip:port> <next_ip:port> <prev_ip:port>", args[0]);
        return Ok(());
    }

    let sender_id: u16 = 1;

    // Load the service account key
    let secret = read_service_account_key("blissful-glass-443215-m6-77d5314689de.json").await?;

    // Create an HTTPS connector
    let https_connector: HttpsConnector<hyper::client::HttpConnector> = HttpsConnectorBuilder::new()
    .with_native_roots()
    .https_or_http()
    .enable_http1()
    .build();

    // Create a hyper client
    let hyper_client: Client<HttpsConnector<hyper::client::HttpConnector>, Body> = Client::builder().build(https_connector);

    // Create the authenticator using the hyper client
    let auth = ServiceAccountAuthenticator::builder(secret)
        .hyper_client(hyper_client.clone())
        .build()
        .await?;

    // Obtain an access token for Google Sheets API
    let token = auth.token(&["https://www.googleapis.com/auth/spreadsheets"]).await?;
    let access_token = token.as_str().to_owned();

    // Wrap the hyper client and token in an Arc for shared use
    let sheets_client = Arc::new(hyper_client);

    let self_addr: SocketAddr = args[1].parse().expect("Invalid self address");
    let next_addr: SocketAddr = args[2].parse().expect("Invalid next address");
    let prev_addr: SocketAddr = args[3].parse().expect("Invalid previous address");
    let all_peers = vec![next_addr, prev_addr];

    // Create sockets and shared state
    let udp_socket = Arc::new(UdpSocket::bind(self_addr).await?);
    let tcp_listener = TcpListener::bind("0.0.0.0:8081").await?;

    let is_leader = Arc::new(Mutex::new(false)); // Tracks if this server is the leader
    let request_count = Arc::new(Mutex::new(0)); // Tracks the number of requests handled

    let election_task = tokio::spawn({
        let udp_socket = Arc::clone(&udp_socket);
        let request_count = Arc::clone(&request_count);
        let is_leader = Arc::clone(&is_leader);
        async move {
            if let Err(e) = election::run_election(sender_id, udp_socket, request_count, is_leader, all_peers).await {
                eprintln!("Error in election task: {:?}", e);
            }
        }
    });

    // Start the client communication task
    let client_comm_task = tokio::spawn({
        // let tcp_listener = Arc::clone(&tcp_listener);
        let is_leader = Arc::clone(&is_leader); // Clone the is_leader flag
        let request_count = Arc::clone(&request_count);
        async move {
            if let Err(e) = client_communication::listen_for_requests(Arc::new(tcp_listener), sheets_client, access_token.to_string(), request_count, is_leader).await {
                eprintln!("Error in client communication task: {:?}", e);
            }
        }
    });

    // Run both tasks concurrently
    tokio::try_join!(election_task, client_comm_task)?;
    Ok(())

    // Start the TCP listener
}

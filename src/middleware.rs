use hyper::{Client, Body};
use hyper_rustls::{HttpsConnector, HttpsConnectorBuilder};
use yup_oauth2::{read_service_account_key, ServiceAccountAuthenticator};
use std::sync::Arc;
use tokio::net::TcpListener;
use std::error::Error;

mod client_communication;

type SheetsClient = Arc<Client<HttpsConnector<hyper::client::HttpConnector>, Body>>;

#[tokio::main(flavor = "multi_thread", worker_threads = 1)]
async fn main() -> Result<(), Box<dyn Error>> {
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
    let access_token = token.as_str();

    // Wrap the hyper client and token in an Arc for shared use
    let sheets_client = Arc::new(hyper_client);

    // Start the TCP listener
    let listener = TcpListener::bind("0.0.0.0:8081").await?;
    client_communication::listen_for_requests(Arc::new(listener), sheets_client, access_token.to_string()).await?;

    Ok(())
}

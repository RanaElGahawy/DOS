use hyper::{Client, Request, Body, Method};
use hyper_rustls::HttpsConnectorBuilder;
use yup_oauth2::{ServiceAccountAuthenticator, ServiceAccountKey};
use tokio::io::{self, AsyncBufReadExt};
use serde_json::json;
use std::fs;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Load service account credentials
    let creds: ServiceAccountKey = serde_json::from_str(
        &fs::read_to_string("blissful-glass-443215-m6-77d5314689de.json").expect("Missing credentials.json file"),
    )?;
    let auth = ServiceAccountAuthenticator::builder(creds)
        .build()
        .await?;

    // Obtain an access token
    let token = auth.token(&["https://www.googleapis.com/auth/spreadsheets"]).await?;
    let access_token = token.as_str();

    // Specify the spreadsheet ID and range
    let spreadsheet_id = "12SqSHonSlPVo8JXcj2Or1cmXOlEPpIjxQ64yZLOHZIg";
    let range = "Sheet1!A:B"; // Adjust based on your sheet structure

    // Read user input
    println!("Enter your name:");
    let stdin = io::BufReader::new(io::stdin());
    let mut input = stdin.lines();

    let name = input.next_line().await?.unwrap_or_default();
    println!("Enter your phone number:");
    let phone = input.next_line().await?.unwrap_or_default();

    if name.is_empty() || phone.is_empty() {
        println!("Both name and phone number are required!");
        return Ok(());
    }

    // Prepare data to append
    let values = json!({
        "values": [[name, phone]]
    });

    // Build the request URL
    let url = format!(
        "https://sheets.googleapis.com/v4/spreadsheets/{}/values/{}:append?valueInputOption=RAW",
        spreadsheet_id, range
    );

    // Create the HTTPS client
    let https = HttpsConnectorBuilder::new()
        .with_native_roots()
        .https_or_http()
        .enable_http1()
        .build();
    let client: Client<_, Body> = Client::builder().build(https);

    // Create the HTTP request
    let req = Request::builder()
        .method(Method::POST)
        .uri(url)
        .header("Authorization", format!("Bearer {}", access_token))
        .header("Content-Type", "application/json")
        .body(Body::from(values.to_string()))?;

    // Send the request
    let res = client.request(req).await?;
    let status = res.status();

    // Check the response status
    if status.is_success() {
        println!("Data added successfully to the Google Sheet!");
    } else {
        println!("Failed to append data: {}", status);
    }

    Ok(())
}

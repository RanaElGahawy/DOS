use hyper::{Body, Client, Method, Request};
use hyper_rustls::HttpsConnector;
use serde_json::json;
use tokio::{io::AsyncWriteExt, sync::Mutex, time::timeout};
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;
use tokio::net::{TcpStream, UdpSocket};
use std::error::Error;

type SheetsClient = Arc<Client<HttpsConnector<hyper::client::HttpConnector>, Body>>;

pub async fn handle_unreachable_id(
    client_id: &str,
    sheets_client: SheetsClient,
    access_token: String,
) -> Result<(), Box<dyn Error>> {
    // Look up the client ID's associated IP in the Google Sheets
    let ip = get_client_ip_from_sheets(&sheets_client, access_token.clone(), client_id).await?;

    if let Some(ip) = ip {
        // Send the UDP "PING" and wait for "ACK" response
        let ping_result = send_udp_ping(&ip).await;

        // Send the result back to the client
        if ping_result {
            println!("IP for client {} is reachable: {}", client_id, ip);
        } else {
            println!("IP for client {} is unreachable: {}", client_id, ip);
            remove_client_from_sheet(&sheets_client, access_token.clone(), client_id).await?;
            
        }
    } else {
        println!("Client {} not found", client_id);
    }

    Ok(())
}

pub async fn send_udp_ping(ip: &str) -> bool {
    // Bind to a local UDP socket
    let socket = UdpSocket::bind("0.0.0.0:0").await.unwrap(); // Local port is automatically chosen

    // Prepare the "PING" message
    let ping_message = b"PING";

    // Send the "PING" message to the target IP (port 12345 for example)
    let target_addr = format!("{}:12345", ip);
    if socket.send_to(ping_message, target_addr).await.is_err() {
        println!("Failed to send PING message to {}", ip);
        return false;
    }

    // Wait for the "ACK" response with a timeout of 2 seconds
    let timeout_duration = Duration::from_secs(2);
    let mut buf = [0; 1024]; // Allocate buffer for receiving the response
    let result = timeout(timeout_duration, socket.recv_from(&mut buf)).await;

    match result {
        Ok(Ok((n, addr))) => {
            let response = String::from_utf8_lossy(&buf[..n]); // Corrected to use the data received
            if response == "ACK" {
                println!("Received ACK from {}", addr);
                true
            } else {
                println!("Received unexpected response from {}: {}", addr, response);
                false
            }
        }
        Ok(Err(e)) => {
            println!("Error receiving response: {}", e);
            false
        }
        Err(_) => {
            println!("Timed out waiting for ACK from {}", ip);
            false
        }
    }
}

pub async fn get_client_ip_from_sheets(
    sheets_client: &SheetsClient,
    access_token: String,
    client_id: &str,
) -> Result<Option<String>, Box<dyn Error>> {
    let spreadsheet_id = "12SqSHonSlPVo8JXcj2Or1cmXOlEPpIjxQ64yZLOHZIg";
    let range = "OnlineClients!A:B"; // Two columns: client_id and client_ip

    let url = format!(
        "https://sheets.googleapis.com/v4/spreadsheets/{}/values/{}",
        spreadsheet_id, range
    );

    let req = Request::builder()
        .method(Method::GET)
        .uri(url)
        .header("Authorization", format!("Bearer {}", access_token)) // Replace with dynamic access token if necessary
        .header("Content-Type", "application/json")
        .body(Body::empty())?;

    let res = sheets_client.request(req).await?;
    let body_bytes = hyper::body::to_bytes(res.into_body()).await?;
    let body_str = String::from_utf8_lossy(&body_bytes);

    // Parse the JSON response and extract the IP for the given client_id
    let data: serde_json::Value = serde_json::from_str(&body_str)?;

    for row in data["values"].as_array().unwrap_or(&vec![]) {
        if let Some(client_id_in_sheet) = row.get(0).and_then(|v| v.as_str()) {
            if client_id_in_sheet == client_id {
                if let Some(client_ip) = row.get(1).and_then(|v| v.as_str()) {
                    return Ok(Some(client_ip.to_string()));
                }
            }
        }
    }

    Ok(None) // Return None if client_id not found
}

pub async fn handle_join_request(
    addr: std::net::SocketAddr,
    sheets_client: SheetsClient,
    access_token: String,
    socket: &mut TcpStream,
) -> Result<(), Box<dyn Error>> {
    let client_id = format!("client_{}", addr.port());
    let ip = addr.ip().to_string();

    add_client_to_dos(&sheets_client, access_token, &client_id.clone(), ip.clone()).await?;
    socket.write_all(client_id.as_bytes()).await?;
    println!("Client joined: {}", client_id);

    Ok(())
}

pub async fn handle_rejoin_request(
    client_id: &str,
    addr: std::net::SocketAddr,
    sheets_client: SheetsClient,
    access_token: String,
    socket: Arc<Mutex<TcpStream>>,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let ip = addr.ip().to_string();

    // Re-add the client to the system
    if readd_client_to_dos(&sheets_client, access_token.clone(), client_id, ip.clone()).await? {
        if let Some(updates) = check_access_rights(&sheets_client, access_token, client_id).await? {
            let response = format!("REJOIN_SUCCESS {}\n", updates.len());
            let mut locked_socket = socket.lock().await;
            locked_socket.write_all(response.as_bytes()).await?;
            locked_socket.flush().await?;
            println!("Rejoin success sent with update count: {}", updates.len());
            
            // Send updates immediately after REJOIN_SUCCESS
            for (image_id, new_access_rights) in updates {
                let update_message = format!("UPDATE {} {}\n", image_id, new_access_rights);
                locked_socket.write_all(update_message.as_bytes()).await?;
                locked_socket.flush().await?;
                println!("Sent update: {}", update_message);
            }
            

        } else {
            let mut locked_socket = socket.lock().await;
            locked_socket.write_all(b"REJOIN_FAILED\n").await?;
            locked_socket.flush().await?;
            println!("Failed to rejoin client: {}", client_id);
        }
    }

    Ok(())
}


pub async fn check_access_rights(
    sheets_client: &SheetsClient,
    access_token: String,
    client_id: &str,
) -> Result<Option<Vec<(String, u8)>>, Box<dyn std::error::Error + Send + Sync>> {
    let spreadsheet_id = "12SqSHonSlPVo8JXcj2Or1cmXOlEPpIjxQ64yZLOHZIg"; // Replace with your spreadsheet ID
    let range = "AccessRights!A:C"; // Columns: client_id, image_id, update

    // Fetch the AccessRights sheet
    let url = format!(
        "https://sheets.googleapis.com/v4/spreadsheets/{}/values/{}",
        spreadsheet_id, range
    );

    let req = Request::builder()
        .method(Method::GET)
        .uri(url)
        .header("Authorization", format!("Bearer {}", access_token))
        .header("Content-Type", "application/json")
        .body(Body::empty())?;

    let res = sheets_client.request(req).await?;
    let body_bytes = hyper::body::to_bytes(res.into_body()).await?;
    let data: serde_json::Value = serde_json::from_slice(&body_bytes)?;

    // Parse the rows to find updates for the client and track row indices
    let mut rows_to_delete = vec![];
    let rows: Vec<(String, u8)> = data
        .get("values")
        .and_then(|v| v.as_array())
        .unwrap_or(&vec![])
        .iter()
        .enumerate()
        .filter_map(|(index, v)| {
            if let (Some(client), Some(image), Some(update)) = (
                v.get(0).and_then(|c| c.as_str()),
                v.get(1).and_then(|c| c.as_str()),
                v.get(2).and_then(|c| c.as_str()),
            ) {
                if client == client_id {
                    rows_to_delete.push(index); // Track rows for deletion
                    if let Ok(update_number) = update.parse::<u8>() {
                        return Some((image.to_string(), update_number));
                    }
                }
            }
            None
        })
        .collect();

    if rows.is_empty() {
        return Ok(None);
    }

    // Prepare the batchUpdate request to delete rows
    let mut requests = vec![];
    for &row_index in rows_to_delete.iter().rev() {
        requests.push(json!({
            "deleteDimension": {
                "range": {
                    "sheetId": 634996676, // Replace with the appropriate sheet ID if not the first sheet
                    "dimension": "ROWS",
                    "startIndex": row_index as i32,
                    "endIndex": (row_index + 1) as i32,
                }
            }
        }));
    }

    // Send the batchUpdate request
    let delete_url = format!(
        "https://sheets.googleapis.com/v4/spreadsheets/{}/:batchUpdate",
        spreadsheet_id
    );

    let delete_request_body = json!({
        "requests": requests
    });

    let delete_req = Request::builder()
        .method(Method::POST)
        .uri(delete_url)
        .header("Authorization", format!("Bearer {}", access_token))
        .header("Content-Type", "application/json")
        .body(Body::from(delete_request_body.to_string()))?;

    let delete_res = sheets_client.request(delete_req).await?;
    if !delete_res.status().is_success() {
        let body_bytes = hyper::body::to_bytes(delete_res.into_body()).await?;
        let body_str = String::from_utf8_lossy(&body_bytes);
        eprintln!("Failed to delete rows. Error: {}", body_str);
    } else {
        println!("Rows deleted successfully.");
    }

    Ok(Some(rows))
}




pub async fn readd_client_to_dos(
    sheets_client: &SheetsClient,
    access_token: String,
    client_id: &str,
    client_ip: String,
) -> Result<bool, Box<dyn std::error::Error + Send + Sync>> {
    let spreadsheet_id = "12SqSHonSlPVo8JXcj2Or1cmXOlEPpIjxQ64yZLOHZIg";
    let range = "OnlineClients!A:B"; // Two columns: client_id and client_ip

    // First, check if the client_id already exists in the sheet
    let url = format!(
        "https://sheets.googleapis.com/v4/spreadsheets/{}/values/{}",
        spreadsheet_id, range
    );

    let req = Request::builder()
        .method(Method::GET)
        .uri(url)
        .header("Authorization", format!("Bearer {}", access_token))
        .header("Content-Type", "application/json")
        .body(Body::empty())?;

    // Send the request to fetch existing data
    let res = sheets_client.request(req).await?;
    let body_bytes = hyper::body::to_bytes(res.into_body()).await?;
    let data: serde_json::Value = serde_json::from_slice(&body_bytes)?;

    // Instead of borrowing the temporary value, make it an owned Vec
    let rows: Vec<Vec<String>> = data
        .get("values")
        .and_then(|v| v.as_array())
        .unwrap_or(&vec![]) // This won't cause the issue anymore
        .iter()
        .filter_map(|v| {
            if let Some(client_id) = v.get(0).and_then(|c| c.as_str()) {
                if let Some(client_ip) = v.get(1).and_then(|c| c.as_str()) {
                    Some(vec![client_id.to_string(), client_ip.to_string()])
                } else {
                    None
                }
            } else {
                None
            }
        })
        .collect();

    let mut client_exists = false;
    let mut row_index = 0;

    // Check if the client_id exists in the rows
    for (index, row) in rows.iter().enumerate() {
        if row[0] == client_id {
            client_exists = true;
            row_index = index + 1; // Spreadsheet is 1-based
            break;
        }
    }

    if client_exists {
        // Update the existing row with new IP
        let update_request = json!({
            "values": [[client_id, client_ip]]
        });

        let update_url = format!(
            "https://sheets.googleapis.com/v4/spreadsheets/{}/values/OnlineClients!A{}:B{}?valueInputOption=RAW",
            spreadsheet_id,
            row_index, // Row to update (1-based index)
            row_index
        );

        let req = Request::builder()
            .method(Method::PUT)
            .uri(update_url)
            .header("Authorization", format!("Bearer {}", access_token))
            .header("Content-Type", "application/json")
            .body(Body::from(update_request.to_string()))?;

        // Send the request to update the row
        let res = sheets_client.request(req).await?;
        if res.status().is_success() {
            println!("Successfully updated client_id: {} with IP: {}", client_id, client_ip);
            return Ok(true);
        } else {
            let body_bytes = hyper::body::to_bytes(res.into_body()).await?;
            let body_str = String::from_utf8_lossy(&body_bytes);
            eprintln!(
                "Failed to update client_id: {} with IP: {}. Error: {}",
                client_id, client_ip, body_str
            );
            return Ok(false);
        }
    } else {
        // If client_id does not exist, append a new row
        let values = json!({
            "values": [[client_id, client_ip]]
        });

        let append_url = format!(
            "https://sheets.googleapis.com/v4/spreadsheets/{}/values/{}:append?valueInputOption=RAW",
            spreadsheet_id, range
        );

        let req = Request::builder()
            .method(Method::POST)
            .uri(append_url)
            .header("Authorization", format!("Bearer {}", access_token))
            .header("Content-Type", "application/json")
            .body(Body::from(values.to_string()))?;

        // Send the request to append the new row
        let res = sheets_client.request(req).await?;
        if res.status().is_success() {
            println!("Successfully added client_id: {} with IP: {}", client_id, client_ip);
            return Ok(true);
        } else {
            let body_bytes = hyper::body::to_bytes(res.into_body()).await?;
            let body_str = String::from_utf8_lossy(&body_bytes);
            eprintln!(
                "Failed to add client_id: {} with IP: {}. Error: {}",
                client_id, client_ip, body_str
            );
            return Ok(false);
        }
    }
}

pub async fn handle_sign_out_request(
    client_id: &str,
    sheets_client: SheetsClient,
    access_token: String,
    socket: &mut TcpStream,
) -> Result<(), Box<dyn Error>> {
    if remove_client_from_sheet(&sheets_client, access_token, client_id).await? {
        socket.write_all(b"ACK").await?;
        println!("Client signed out: {}", client_id);
    } else {
        socket.write_all(b"NAK").await?;
        println!("Failed to sign out client: {}", client_id);
    }
    Ok(())
}

pub async fn add_client_to_dos(
    sheets_client: &SheetsClient,
    access_token: String,
    client_id: &str,
    client_ip: String, // Updated parameter name for clarity
) -> Result<bool, Box<dyn Error>> {
    let spreadsheet_id = "12SqSHonSlPVo8JXcj2Or1cmXOlEPpIjxQ64yZLOHZIg";
    let range = "OnlineClients!A:B"; // Two columns: client_id and client_ip

    // Prepare the data to append
    let values = json!({
        "values": [[client_id, client_ip]]
    });

    // Build the request URL for appending data
    let url = format!(
        "https://sheets.googleapis.com/v4/spreadsheets/{}/values/{}:append?valueInputOption=RAW",
        spreadsheet_id, range
    );

    // Create the HTTP request to append data
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
        println!("Successfully added client_id: {} with IP: {}", client_id, client_ip);
        Ok(true)
    } else {
        let body_bytes = hyper::body::to_bytes(res.into_body()).await?;
        let body_str = String::from_utf8_lossy(&body_bytes);
        eprintln!(
            "Failed to add client_id: {} with IP: {}. Error: {}",
            client_id, client_ip, body_str
        );
        Ok(false)
    }
}

pub async fn remove_client_from_sheet(
    sheets_client: &SheetsClient,
    access_token: String,
    client_id: &str,
) -> Result<bool, Box<dyn Error>> {
    let spreadsheet_id = "12SqSHonSlPVo8JXcj2Or1cmXOlEPpIjxQ64yZLOHZIg";
    let range = "OnlineClients!A:C"; // Adjust this range to match your sheet structure.

    // Fetch the data to locate the client ID
    let url = format!(
        "https://sheets.googleapis.com/v4/spreadsheets/{}/values/{}",
        spreadsheet_id, range
    );

    let req = Request::builder()
        .method(Method::GET)
        .uri(url)
        .header("Authorization", format!("Bearer {}", access_token))
        .header("Content-Type", "application/json")
        .body(Body::empty())?;

    let res = sheets_client.request(req).await?;
    if !res.status().is_success() {
        let body_bytes = hyper::body::to_bytes(res.into_body()).await?;
        let body_str = String::from_utf8_lossy(&body_bytes);
        eprintln!("Failed to fetch Google Sheets data: {}", body_str);
        return Ok(false);
    }

    let body_bytes = hyper::body::to_bytes(res.into_body()).await?;
    let data: serde_json::Value = serde_json::from_slice(&body_bytes)?;

    if let Some(rows) = data.get("values").and_then(|v| v.as_array()) {
        for (index, row) in rows.iter().enumerate() {
            if let Some(existing_id) = row.get(0).and_then(|v| v.as_str()) {
                if existing_id == client_id {
                    // Found the client ID, delete the row
                    let delete_request = json!({
                        "requests": [
                            {
                                "deleteRange": {
                                    "range": {
                                        "sheetId": 0, // Adjust the sheetId based on your sheet
                                        "startRowIndex": index,
                                        "endRowIndex": index + 1
                                    },
                                    "shiftDimension": "ROWS"
                                }
                            }
                        ]
                    });

                    let batch_url = format!(
                        "https://sheets.googleapis.com/v4/spreadsheets/{}/:batchUpdate",
                        spreadsheet_id
                    );

                    let req = Request::builder()
                        .method(Method::POST)
                        .uri(batch_url)
                        .header("Authorization", format!("Bearer {}", access_token))
                        .header("Content-Type", "application/json")
                        .body(Body::from(delete_request.to_string()))?;

                    let res = sheets_client.request(req).await?;
                    if res.status().is_success() {
                        println!("Successfully deleted row for client ID: {}", client_id);
                        return Ok(true);
                    } else {
                        let body_bytes = hyper::body::to_bytes(res.into_body()).await?;
                        let body_str = String::from_utf8_lossy(&body_bytes);
                        eprintln!("Failed to delete row: {}", body_str);
                        return Ok(false);
                    }
                }
            }
        }
    }

    println!("Client ID not found in Google Sheets: {}", client_id);
    Ok(false)
}

pub async fn handle_show_active_clients_request(
    requesting_addr: std::net::SocketAddr,
    sheets_client: SheetsClient,
    access_token: String,
    socket: &mut TcpStream,
) -> Result<(), Box<dyn Error>> {
    let spreadsheet_id = "12SqSHonSlPVo8JXcj2Or1cmXOlEPpIjxQ64yZLOHZIg";
    let range = "OnlineClients!A:B"; // Two columns: client_id and ip

    // Fetch data from Google Sheets
    let url = format!(
        "https://sheets.googleapis.com/v4/spreadsheets/{}/values/{}",
        spreadsheet_id, range
    );

    let req = Request::builder()
        .method(Method::GET)
        .uri(url)
        .header("Authorization", format!("Bearer {}", access_token))
        .header("Content-Type", "application/json")
        .body(Body::empty())?;

    let res = sheets_client.request(req).await?;
    if !res.status().is_success() {
        let body_bytes = hyper::body::to_bytes(res.into_body()).await?;
        let body_str = String::from_utf8_lossy(&body_bytes);
        eprintln!("Failed to fetch Google Sheets data: {}", body_str);
        return Ok(());
    }

    let body_bytes = hyper::body::to_bytes(res.into_body()).await?;
    let data: serde_json::Value = serde_json::from_slice(&body_bytes)?;

    // Parse active clients, skipping the first row (titles)
    let mut active_clients: HashMap<String, String> = HashMap::new();
    if let Some(rows) = data.get("values").and_then(|v| v.as_array()) {
        let requesting_ip = requesting_addr.ip().to_string();

        for (i, row) in rows.iter().enumerate() {
            // Skip the first row containing titles
            if i == 0 {
                continue;
            }

            if let (Some(client_id), Some(ip)) = (row.get(0), row.get(1)) {
                if ip.as_str() != Some(&requesting_ip) {
                    active_clients.insert(
                        client_id.as_str().unwrap_or_default().to_string(),
                        ip.as_str().unwrap_or_default().to_string(),
                    );
                }
            }
        }
    }

    // Send active clients as JSON response
    let response = serde_json::to_string(&active_clients)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e))?;

    socket.write_all(response.as_bytes()).await?;
    println!(
        "Sent active clients (excluding {}): {}",
        requesting_addr, response
    );

    Ok(())
}
pub async fn notify_update_failure(
    client_id: &str,
    image_name: &str,
    new_access_rights: u8,
    sheets_client: SheetsClient,
    access_token: String,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let spreadsheet_id = "12SqSHonSlPVo8JXcj2Or1cmXOlEPpIjxQ64yZLOHZIg";
    let range = "AccessRights!A:C"; // Adjust as needed

    // Prepare the data to append
    let values = serde_json::json!({
        "values": [[client_id, image_name, new_access_rights.to_string()]]
    });

    let url = format!(
        "https://sheets.googleapis.com/v4/spreadsheets/{}/values/{}:append?valueInputOption=RAW",
        spreadsheet_id, range
    );

    // Create the HTTP request to append data
    let req = hyper::Request::builder()
        .method(hyper::Method::POST)
        .uri(url)
        .header("Authorization", format!("Bearer {}", access_token))
        .header("Content-Type", "application/json")
        .body(hyper::Body::from(values.to_string()))?;

    // Send the request
    let res = sheets_client.request(req).await?;

    // Handle the response
    if res.status().is_success() {
        println!(
            "Successfully logged UPDATE failure for client_id: {}, image_name: {}, new_access_rights: {}",
            client_id, image_name, new_access_rights
        );
        Ok(())
    } else {
        let body_bytes = hyper::body::to_bytes(res.into_body()).await?;
        let error_message = String::from_utf8_lossy(&body_bytes);
        Err(format!("Failed to log UPDATE failure: {}", error_message).into())
    }
}

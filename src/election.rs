use tokio::net::UdpSocket;
use std::sync::Arc;
use tokio::sync::Mutex;
use tokio::time::{self, Duration};
use std::net::SocketAddr;
use tokio::sync::watch;
use std::io;

const ELECTION_TIMEOUT: Duration = Duration::from_secs(6);
const HEARTBEAT_INTERVAL: Duration = Duration::from_secs(2);
const TRIGGER_ELECTION: Duration = Duration::from_secs(6);

pub async fn run_election(
   self_id: u16,
   udp_socket: Arc<UdpSocket>,
   request_count: Arc<Mutex<u32>>,
   has_token: Arc<Mutex<bool>>,
   all_peers: Vec<SocketAddr>, // List of all other servers
) -> Result<(), Box<dyn std::error::Error>> {
   let mut buffer = [0; 1024];
   let mut election_timeout = time::interval(ELECTION_TIMEOUT);
   let mut heartbeat_interval = time::interval(HEARTBEAT_INTERVAL);
   let mut trigger_election_timer = time::interval(TRIGGER_ELECTION);

   let (election_reset_tx, mut election_reset_rx) = watch::channel(false);

   loop {
       tokio::select! {
           // Handle incoming messages
           result = udp_socket.recv_from(&mut buffer) => {
            match result {
                Ok((size, src)) => {
                        let message = String::from_utf8_lossy(&buffer[..size]).to_string();

                        if message.starts_with("ELECTION") {
                            let parts: Vec<&str> = message.split_whitespace().collect();
                            let sender_request_count: u32 = parts[1].parse()?;
                            let sender_id: u16 = parts[2].parse()?; // Simulating ID with a number
                            let self_request_count = *request_count.lock().await;

                            if sender_request_count > self_request_count
                                || (sender_request_count == self_request_count && sender_id < self_id)
                            {
                                // Respond with own candidacy
                                let response = format!("CANDIDATE {} {}", self_request_count, self_id);
                                for peer in &all_peers {
                                    udp_socket.send_to(response.as_bytes(), *peer).await?;
                                }
                            }
                        } else if message.starts_with("CANDIDATE") {
                            // Compare candidates and determine if this server should concede
                            let parts: Vec<&str> = message.split_whitespace().collect();
                            let candidate_request_count: u32 = parts[1].parse()?;
                            let candidate_id: u16 = parts[2].parse()?;
                            let self_request_count = *request_count.lock().await;

                            if candidate_request_count < self_request_count
                                || (candidate_request_count == self_request_count && candidate_id > self_id)
                            {
                                // Concede leadership to the better candidate
                                *has_token.lock().await = false;
                                election_timeout = time::interval(ELECTION_TIMEOUT);
                                election_timeout.tick().await;
                            } else
                            {
                                // Respond with own candidacy
                                *has_token.lock().await = true;
                                let response = format!("LEADER {}", self_id);
                                election_timeout = time::interval(ELECTION_TIMEOUT);
                                election_timeout.tick().await;
                                trigger_election_timer = time::interval(TRIGGER_ELECTION);
                                trigger_election_timer.tick().await;
                                for peer in &all_peers {
                                    udp_socket.send_to(response.as_bytes(), *peer).await?;
                                }
                            }
                        } else if message.starts_with("LEADER") {
                            // Update leader status
                            let parts: Vec<&str> = message.split_whitespace().collect();
                            let leader_id: u16 = parts[1].parse()?;
                            println!("New leader announced: {}", leader_id);
                            // Update local leader state
                            let mut token = has_token.lock().await;
                            *token = leader_id == self_id;
                            election_timeout = time::interval(ELECTION_TIMEOUT);
                            election_timeout.tick().await;

                        } else if message.starts_with("HEARTBEAT") {
                            // Reset leader timeout on receiving a heartbeat
                            println!("Heartbeat received from leader: {}", src);
                            election_timeout = time::interval(ELECTION_TIMEOUT);
                            election_timeout.tick().await;
                        }
                    },
                    Err(e) if e.kind() == io::ErrorKind::AddrNotAvailable => {
                        println!("Warning: Cannot receive from socket. Error: {}", e);
                        // Ignore this error
                    },
                    Err(e) => {
                        println!("Unexpected error while receiving: {}", e);
                        return Err(Box::new(e));
                    },
                }
           }

           // Trigger election if timeout occurs (leader failure detected)
           _ = election_timeout.tick() => {
               if !*has_token.lock().await {
                   println!("Leader timeout detected. Starting new election.");
                   trigger_election(self_id, udp_socket.clone(), request_count.clone(), all_peers.clone(), election_reset_tx.clone(), has_token.clone()).await?;
               }
           }

           // Reset election timeout on signal
           _ = election_reset_rx.changed() => {
               if *election_reset_rx.borrow() {
                   println!("reseting timeout");
                   election_timeout = time::interval(ELECTION_TIMEOUT);
                   election_timeout.tick().await;
                   trigger_election_timer = time::interval(TRIGGER_ELECTION);
                   trigger_election_timer.tick().await;
               }
           }

           // Trigger election after 2 seconds if it is the current leader
           _ = trigger_election_timer.tick() => {
               if *has_token.lock().await {
                   println!("Choose new leader. Starting new election.");
                   election_timeout = time::interval(ELECTION_TIMEOUT);
                   election_timeout.tick().await;
                   trigger_election(self_id, udp_socket.clone(), request_count.clone(), all_peers.clone(), election_reset_tx.clone(), has_token.clone()).await?;
               }
           }

           // If this server is the leader, send periodic heartbeats
           _ = heartbeat_interval.tick() => {
               if *has_token.lock().await {
                   println!("Leader {} sending heartbeat.", self_id);
                   let heartbeat_message = "HEARTBEAT".to_string();
                   for peer in &all_peers {
                       udp_socket.send_to(heartbeat_message.as_bytes(), peer).await?;
                   }
               }
           }
       }
   }
}

async fn try_send_to(
    udp_socket: Arc<UdpSocket>,
    message: &[u8],
    peer: SocketAddr,
) -> Result<(), ()> {
    match udp_socket.send_to(message, peer).await {
        Ok(_) => Ok(()),
        Err(e) if e.kind() == io::ErrorKind::AddrNotAvailable => {
            println!("Warning: Could not send to {}. Error: {}", peer, e);
            Err(()) // Ignore this error
        }
        Err(e) => {
            println!("Unexpected error while sending to {}: {}", peer, e);
            Err(()) // Handle other errors as necessary
        }
    }
}

pub async fn trigger_election(
   self_id: u16,
   udp_socket: Arc<UdpSocket>,
   request_count: Arc<Mutex<u32>>,
   all_peers: Vec<SocketAddr>,
   election_reset: watch::Sender<bool>, // Sender to notify reset
   has_token: Arc<Mutex<bool>>,
) -> Result<(), Box<dyn std::error::Error>> {
   let self_request_count = *request_count.lock().await;


   // Broadcast election messages to all peers
   for peer in &all_peers {
       let message = format!("ELECTION {} {}", self_request_count, self_id);
       if let Err(_) = try_send_to(udp_socket.clone(), message.as_bytes(), *peer).await {
            println!("Skipping peer {} due to an error.", peer);
        }
   }

   println!("Election started by {}. Waiting for responses...", self_id);

   let timeout = Duration::from_secs(3);
   let mut buffer = [0; 1024];
   let start_time = tokio::time::Instant::now();

   while tokio::time::Instant::now().duration_since(start_time) < timeout {
       if let Ok((size, _)) = udp_socket.try_recv_from(&mut buffer) {
           let message = String::from_utf8_lossy(&buffer[..size]).to_string();
           if message.starts_with("ELECTION") {
            let parts: Vec<&str> = message.split_whitespace().collect();
            let sender_request_count: u32 = parts[1].parse()?;
            let sender_id: u16 = parts[2].parse()?; // Simulating ID with a number
            let self_request_count = *request_count.lock().await;

            if sender_request_count > self_request_count
                || (sender_request_count == self_request_count && sender_id > self_id)
                {
                    println!("Found Better Candidate: {}", sender_id);
                    {
                        let mut token = has_token.lock().await;
                        *token = false; // Relinquish leadership
                    }
                    return Ok(()); // Exit election if a response is received
                }
           }
           else if message.starts_with("CANDIDATE") || message.starts_with("LEADER") {
               println!("Received response: {}", message);
               return Ok(()); // Exit election if a response is received
           }
       }
       tokio::task::yield_now().await; // Allow other tasks to run
   }

   // No responses received: Assume leadership
//    println!("No responses received. Server {} assumes leadership.", self_id);
   let response = format!("LEADER {}", self_id);
   {
       let mut token = has_token.lock().await;
       *token = true; // Mark this server as the leader
   }

   for peer in &all_peers {
       udp_socket.send_to(response.as_bytes(), *peer).await?;
   }

   // Notify the main loop to reset the timeout
   let _ = election_reset.send(true);

   Ok(())
}
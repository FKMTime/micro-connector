use fastwebsockets::upgrade;
use fastwebsockets::OpCode;
use fastwebsockets::WebSocketError;
use hyper::server::conn::Http;
use hyper::service::service_fn;
use hyper::Body;
use hyper::Request;
use hyper::Response;
use tokio::net::TcpListener;

#[derive(serde::Deserialize, Debug)]
#[serde(untagged)]
enum TimerResponse {
    Connect {
        esp_id: String,
    },
    Solve {
        solve_time: u128,
        card_id: u128,
        esp_id: String,
        timestamp: u128,
        session_id: i64,
    },
}

async fn handle_client(fut: upgrade::UpgradeFut) -> Result<(), WebSocketError> {
    let mut ws = fastwebsockets::FragmentCollector::new(fut.await?);

    loop {
        let frame = ws.read_frame().await?;
        match frame.opcode {
            OpCode::Close => break,
            OpCode::Text | OpCode::Binary => {
                let response: TimerResponse = serde_json::from_slice(&frame.payload).unwrap();
                println!("Received: {:?}", response);
                ws.write_frame(frame).await?;
            }
            _ => {}
        }
    }

    Ok(())
}
async fn server_upgrade(mut req: Request<Body>) -> Result<Response<Body>, WebSocketError> {
    let (response, fut) = upgrade::upgrade(&mut req)?;

    tokio::task::spawn(async move {
        if let Err(e) = tokio::task::unconstrained(handle_client(fut)).await {
            eprintln!("Error in websocket connection: {}", e);
        }
    });

    Ok(response)
}

#[tokio::main]
async fn main() -> Result<(), WebSocketError> {
    let listener = TcpListener::bind("0.0.0.0:8080").await?;
    println!("Server started, listening on {}", "0.0.0.0:8080");
    loop {
        let (stream, _) = listener.accept().await?;
        println!("Client connected");
        tokio::spawn(async move {
            let conn_fut = Http::new()
                .serve_connection(stream, service_fn(server_upgrade))
                .with_upgrades();
            if let Err(e) = conn_fut.await {
                println!("An error occurred: {:?}", e);
            }
        });
    }
}

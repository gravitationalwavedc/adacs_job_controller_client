use crate::messaging::{Message, Priority, SERVER_READY, SYSTEM_SOURCE};
use futures_util::{SinkExt, StreamExt};
use tokio::net::TcpListener;
use tokio::sync::mpsc;
use tokio_tungstenite::{
    accept_async,
    tungstenite::{self, protocol::Message as WsMessage},
};

pub struct WebsocketServerFixture {
    pub port: u16,
    pub msg_rx: mpsc::UnboundedReceiver<Message>,
    pub msg_tx: mpsc::UnboundedSender<Vec<u8>>,
    _handle: tokio::task::JoinHandle<()>,
}

impl WebsocketServerFixture {
    pub async fn new() -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();

        let (msg_tx_to_test, msg_rx_from_server) = mpsc::unbounded_channel::<Message>();
        let (msg_tx_to_server, mut msg_rx_from_test) = mpsc::unbounded_channel::<Vec<u8>>();

        let handle = tokio::spawn(async move {
            if let Ok((stream, _)) = listener.accept().await {
                let mut ws_stream = accept_async(stream)
                    .await
                    .expect("Error during the websocket handshake occurred");

                // Send SERVER_READY
                let ready_msg = Message::new(SERVER_READY, Priority::Highest, SYSTEM_SOURCE);
                ws_stream
                    .send(WsMessage::Binary(ready_msg.get_data().clone().into()))
                    .await
                    .unwrap();

                let (ws_sender, ws_receiver) = ws_stream.split();

                let mut ws_receiver = ws_receiver;
                let mut ws_sender = ws_sender;

                let msg_tx_to_test = msg_tx_to_test;

                tokio::select! {
                    res = async {
                        while let Some(msg) = ws_receiver.next().await {
                            let msg = match msg {
                                Ok(msg) => msg,
                                Err(
                                    tungstenite::Error::ConnectionClosed
                                    | tungstenite::Error::Protocol(
                                        tungstenite::error::ProtocolError::ResetWithoutClosingHandshake,
                                    ),
                                ) => break,
                                Err(e) => panic!("WS error: {e}"),
                            };
                            if msg.is_binary() {
                                let m = Message::from_data(msg.into_data().to_vec());
                                msg_tx_to_test.send(m).unwrap();
                            }
                        }
                    } => res,
                    res = async {
                        while let Some(data) = msg_rx_from_test.recv().await {
                            ws_sender.send(WsMessage::Binary(data.into())).await.unwrap();
                        }
                    } => res,
                }
            }
        });

        Self {
            port,
            msg_rx: msg_rx_from_server,
            msg_tx: msg_tx_to_server,
            _handle: handle,
        }
    }

    pub fn get_url(&self) -> String {
        format!("ws://127.0.0.1:{}", self.port)
    }
}

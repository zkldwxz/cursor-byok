//! Holds the one connection to the computer, for as long as the page wants it.
use std::sync::Mutex;

use remote_link::{Link, Phone, Route, Ticket};
use serde::Serialize;
use tokio::sync::mpsc;

#[derive(Default)]
pub struct Connection(Mutex<Current>);

#[derive(Default)]
struct Current {
    id: u32,
    /// Dropping it hangs up.
    outbox: Option<mpsc::UnboundedSender<String>>,
    route: Option<Route>,
}

/// What the page hears: a message, or nothing more once the computer is gone.
#[derive(Debug, PartialEq, Serialize)]
pub struct Heard {
    pub message: Option<String>,
}

impl Connection {
    /// Replaces any earlier connection. `hear` answers whether the page still listens.
    pub async fn dial(
        &self,
        ticket: &str,
        hear: impl Fn(Heard) -> bool + Send + Sync + 'static,
    ) -> remote_link::Result<u32> {
        let ticket = Ticket::parse(ticket)?;
        let id = {
            let mut current = self.0.lock().unwrap();
            current.outbox = None;
            current.id += 1;
            current.id
        };
        let phone = Phone::bind(ticket.relay.as_ref()).await?;
        let link = phone.dial(&ticket).await?;
        let route = link.route();
        let (outbox, outgoing) = mpsc::unbounded_channel();
        {
            let mut current = self.0.lock().unwrap();
            // Dialled again meanwhile: this one ends as soon as it begins.
            if current.id == id {
                current.outbox = Some(outbox);
                current.route = Some(route);
            }
        }
        tokio::spawn(async move {
            carry(link, outgoing, &hear).await;
            hear(Heard { message: None });
            phone.close().await;
        });
        Ok(id)
    }

    pub fn say(&self, id: u32, text: String) {
        let current = self.0.lock().unwrap();
        if let (true, Some(outbox)) = (current.id == id, &current.outbox) {
            // Gone already: the page hears so by itself.
            let _ = outbox.send(text);
        }
    }

    /// Whether the computer is reached without the relay; `None` while it is not reached.
    pub fn direct(&self) -> Option<bool> {
        let current = self.0.lock().unwrap();
        let outbox = current.outbox.as_ref()?;
        (!outbox.is_closed()).then(|| current.route.as_ref().map(Route::direct))?
    }

    pub fn hang_up(&self, id: u32) {
        let mut current = self.0.lock().unwrap();
        if current.id == id {
            current.outbox = None;
        }
    }
}

async fn carry(
    mut link: Link,
    mut outgoing: mpsc::UnboundedReceiver<String>,
    hear: &impl Fn(Heard) -> bool,
) {
    loop {
        tokio::select! {
            message = link.receive() => {
                let heard = Heard { message };
                if heard.message.is_none() || !hear(heard) {
                    return;
                }
            }
            text = outgoing.recv() => match text {
                Some(text) if link.send(&text).await.is_ok() => {}
                _ => return,
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    type Heards = mpsc::UnboundedReceiver<Heard>;

    async fn dial(connection: &Connection, ticket: &Ticket) -> (u32, Heards) {
        let (heard, heards) = mpsc::unbounded_channel();
        let id = connection
            .dial(&ticket.link(), move |it| heard.send(it).is_ok())
            .await
            .unwrap();
        (id, heards)
    }

    async fn reached() -> (remote_link::Host, Connection, Link, u32, Heards) {
        let host = remote_link::Host::bind(None, &remote_link::Identity::generate())
            .await
            .unwrap();
        let connection = Connection::default();
        let ticket = host.ticket("code".into());
        let answer = async { host.accept().await.unwrap().answer().await.unwrap() };
        let ((id, heards), link) = tokio::join!(dial(&connection, &ticket), answer);
        (host, connection, link, id, heards)
    }

    #[tokio::test]
    async fn the_page_and_the_computer_hear_each_other() {
        let (_host, connection, mut computer, id, mut page) = reached().await;
        connection.say(id, "hello".into());
        assert_eq!(computer.receive().await.unwrap(), "hello");
        computer.send("hi").await.unwrap();
        assert_eq!(page.recv().await.unwrap().message.unwrap(), "hi");
        assert_eq!(connection.direct(), Some(true));
        connection.hang_up(id);
        assert_eq!(connection.direct(), None);
    }

    #[tokio::test]
    async fn the_page_hears_that_the_computer_is_gone() {
        let (_host, _connection, computer, _, mut page) = reached().await;
        drop(computer);
        assert_eq!(page.recv().await.unwrap(), Heard { message: None });
    }

    #[tokio::test]
    async fn hanging_up_and_dialling_again_end_the_connection_before() {
        let (_host, connection, mut computer, id, _page) = reached().await;
        connection.hang_up(id + 1);
        connection.say(id, "still here".into());
        assert_eq!(computer.receive().await.unwrap(), "still here");
        connection.hang_up(id);
        assert!(computer.receive().await.is_none());

        let (host, connection, mut before, old, _page) = reached().await;
        let ticket = host.ticket("code".into());
        let answer = async { host.accept().await.unwrap().answer().await.unwrap() };
        let ((id, _heards), mut now) = tokio::join!(dial(&connection, &ticket), answer);
        assert!(before.receive().await.is_none());
        connection.say(old, "lost".into());
        connection.say(id, "kept".into());
        assert_eq!(now.receive().await.unwrap(), "kept");
    }

    #[tokio::test]
    async fn what_is_no_pairing_link_is_refused() {
        let connection = Connection::default();
        let dialled = connection.dial("https://example.com", |_| true).await;
        assert!(dialled.is_err());
    }
}

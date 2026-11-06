use async_channel::Sender;

use super::ChannelEventListener;
use crate::event::Event as TerminalEvent;

pub struct ChannelEventListenerBuilder<E = TerminalEvent> {
    wakeups_tx: Option<Sender<()>>,
    events_tx: Option<Sender<E>>,
}
impl<E> ChannelEventListenerBuilder<E>
where
    E: From<TerminalEvent> + Send + 'static,
{
    fn new() -> Self {
        ChannelEventListenerBuilder {
            wakeups_tx: None,
            events_tx: None,
        }
    }

    pub fn with_wakeups_tx(mut self, wakeups_tx: Sender<()>) -> Self {
        self.wakeups_tx = Some(wakeups_tx);
        self
    }

    pub fn with_terminal_events_tx(mut self, events_tx: Sender<E>) -> Self {
        self.events_tx = Some(events_tx);
        self
    }

    pub fn build(self) -> ChannelEventListener {
        let (pty_bytes_read_tx, _) = async_broadcast::broadcast(1);
        ChannelEventListener::new(
            self.wakeups_tx.unwrap_or_else(|| {
                let (tx, _) = async_channel::unbounded();
                tx
            }),
            self.events_tx.unwrap_or_else(|| {
                let (tx, _) = async_channel::unbounded();
                tx
            }),
            pty_bytes_read_tx,
        )
    }
}

impl ChannelEventListener {
    pub fn new_for_test() -> Self {
        ChannelEventListenerBuilder::<TerminalEvent>::new().build()
    }

    pub fn builder_for_test<E>() -> ChannelEventListenerBuilder<E>
    where
        E: From<TerminalEvent> + Send + 'static,
    {
        ChannelEventListenerBuilder::new()
    }
}

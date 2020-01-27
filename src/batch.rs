// Source: https://github.com/mre/tokio-batch/blob/master/src/lib.rs

use futures::stream::{Fuse, FusedStream, Stream};
use futures::task::{Context, Poll};
use futures::Future;
use futures::StreamExt;
use std::mem;
use std::pin::Pin;
use std::time::Duration;
use tokio::time::Delay;

pub trait ChunksTimeoutStreamExt: Stream {
    fn chunks_timeout(self, capacity: usize, duration: Duration) -> ChunksTimeout<Self>
    where
        Self: Sized + Unpin,
    {
        ChunksTimeout::new(self, capacity, duration)
    }
}
impl<T: ?Sized> ChunksTimeoutStreamExt for T where T: Stream {}

#[derive(Debug)]
#[must_use = "streams do nothing unless polled"]
pub struct ChunksTimeout<St: Stream> {
    stream: Fuse<St>,
    items: Vec<St::Item>,
    cap: usize,
    // https://github.com/rust-lang-nursery/futures-rs/issues/1475
    clock: Option<Delay>,
    duration: Duration,
}

impl<St: Unpin + Stream> Unpin for ChunksTimeout<St> {}

impl<St: Stream> ChunksTimeout<St>
where
    St: Stream + Unpin,
{
    pub fn new(stream: St, capacity: usize, duration: Duration) -> ChunksTimeout<St> {
        assert!(capacity > 0);

        ChunksTimeout {
            stream: stream.fuse(),
            items: Vec::with_capacity(capacity),
            cap: capacity,
            clock: None,
            duration,
        }
    }

    fn take(mut self: Pin<&mut Self>) -> Vec<St::Item> {
        let cap = self.cap;
        mem::replace(&mut self.items, Vec::with_capacity(cap))
    }
}

impl<St> Stream for ChunksTimeout<St>
where
    St: Stream + Unpin,
{
    type Item = Vec<St::Item>;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        loop {
            match Stream::poll_next(Pin::new(&mut self.stream), cx) {
                Poll::Ready(item) => match item {
                    // Push the item into the buffer and check whether it is full.
                    // If so, replace our buffer with a new and empty one and return
                    // the full one.
                    Some(item) => {
                        if self.items.is_empty() {
                            self.clock = Some(tokio::time::delay_for(self.duration));
                        }
                        self.items.push(item);
                        if self.items.len() >= self.cap {
                            self.clock = None;
                            return Poll::Ready(Some(ChunksTimeout::take(self)));
                        } else {
                            // Continue the loop
                            continue;
                        }
                    }

                    // Since the underlying stream ran out of values, return what we
                    // have buffered, if we have anything.
                    None => {
                        let last = if self.items.is_empty() {
                            None
                        } else {
                            let full_buf = mem::replace(&mut self.items, Vec::new());
                            Some(full_buf)
                        };

                        return Poll::Ready(last);
                    }
                },
                // Don't return here, as we need to need check the clock.
                Poll::Pending => {}
            }

            match self
                .clock
                .as_mut()
                .map(|clock| Future::poll(Pin::new(clock), cx))
            {
                Some(Poll::Ready(())) => {
                    self.clock = None;
                    return Poll::Ready(Some(ChunksTimeout::take(self)));
                }
                Some(Poll::Pending) => {}
                None => {
                    debug_assert!(
                        self.items.is_empty(),
                        "Inner buffer is empty, but clock is available."
                    );
                }
            }

            return Poll::Pending;
        }
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        let chunk_len = if self.items.is_empty() { 0 } else { 1 };
        let (lower, upper) = self.stream.size_hint();
        let lower = lower.saturating_add(chunk_len);
        let upper = match upper {
            Some(x) => x.checked_add(chunk_len),
            None => None,
        };
        (lower, upper)
    }
}

impl<St> FusedStream for ChunksTimeout<St>
where
    St: FusedStream + Unpin,
{
    fn is_terminated(&self) -> bool {
        self.stream.is_terminated() & self.items.is_empty()
    }
}

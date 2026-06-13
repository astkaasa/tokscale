use std::sync::mpsc;

#[derive(Debug)]
pub(crate) enum BackgroundJobPoll<T> {
    Ready(T),
    Disconnected,
}

#[derive(Debug)]
pub(crate) struct BackgroundJob<T> {
    rx: Option<mpsc::Receiver<T>>,
}

impl<T> Default for BackgroundJob<T> {
    fn default() -> Self {
        Self { rx: None }
    }
}

impl<T: Send + 'static> BackgroundJob<T> {
    pub(crate) fn is_running(&self) -> bool {
        self.rx.is_some()
    }

    pub(crate) fn start<F>(&mut self, work: F) -> bool
    where
        F: FnOnce() -> T + Send + 'static,
    {
        if self.rx.is_some() {
            return false;
        }

        let (tx, rx) = mpsc::channel();
        self.rx = Some(rx);
        std::thread::spawn(move || {
            let _ = tx.send(work());
        });
        true
    }

    pub(crate) fn poll(&mut self) -> Option<BackgroundJobPoll<T>> {
        let rx = self.rx.as_ref()?;
        match rx.try_recv() {
            Ok(value) => {
                self.rx = None;
                Some(BackgroundJobPoll::Ready(value))
            }
            Err(mpsc::TryRecvError::Disconnected) => {
                self.rx = None;
                Some(BackgroundJobPoll::Disconnected)
            }
            Err(mpsc::TryRecvError::Empty) => None,
        }
    }
}

use super::{News, TreeHandle, TreeItem};
use std::sync::atomic::Ordering;

impl TreeHandle {
    pub(super) fn finish_for_test(&mut self) -> Result<(), String> {
        let bytes = TreeItem::Finish.bytes().map_err(|err| err.to_string())?;
        self.bytes.fetch_add(bytes, Ordering::AcqRel);
        self.waiting.fetch_add(1, Ordering::AcqRel);
        if let Err(err) = self.sender.send(TreeItem::Finish) {
            self.bytes.fetch_sub(bytes, Ordering::AcqRel);
            self.waiting.fetch_sub(1, Ordering::AcqRel);
            return Err(format!("the tree thread has stopped: {err}"));
        }
        loop {
            match self.news.recv() {
                Ok(News::Finished(result)) => {
                    if let Some(join) = self.join.take() {
                        join.join()
                            .map_err(|_| "the tree thread panicked".to_string())?;
                    }
                    if let Err(error) = &result {
                        self.error = Some(error.clone());
                    }
                    return result;
                }
                Ok(news) => self.hear(news),
                Err(err) => return Err(format!("waiting for the tree to finish: {err}")),
            }
        }
    }
}

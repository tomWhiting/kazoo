use super::MicFacts;
use crate::Result;

#[derive(Debug)]
pub(super) struct FactsCache<I> {
    opened: Option<(I, MicFacts)>,
}

impl<I> Default for FactsCache<I> {
    fn default() -> Self {
        Self { opened: None }
    }
}

impl<I: PartialEq> FactsCache<I> {
    pub(super) fn read(
        &mut self,
        identity: I,
        probe: impl FnOnce() -> Result<MicFacts>,
    ) -> Result<MicFacts> {
        if let Some((opened, facts)) = &self.opened {
            if *opened == identity {
                return Ok(facts.clone());
            }
        }
        let facts = probe()?;
        self.opened = Some((identity, facts.clone()));
        Ok(facts)
    }
    pub(super) fn invalidate(&mut self) {
        self.opened = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;
    #[test]
    fn mouth_mic_watch_probes_configuration_once_over_100_frames() {
        let mut cache = FactsCache::<u8>::default();
        let probes = Cell::new(0);
        let mut read = |identity| {
            cache.read(identity, || {
                probes.set(probes.get() + 1);
                Ok(MicFacts {
                    name: "mic".into(),
                    rate: 48_000,
                    channels: 1,
                })
            })
        };
        for _ in 0..100 {
            assert_eq!(read(1).unwrap().rate, 48_000);
        }
        assert_eq!(probes.get(), 1);
        assert_eq!(read(2).unwrap().rate, 48_000);
        assert_eq!(probes.get(), 2);
        cache.invalidate();
        cache
            .read(2, || {
                probes.set(probes.get() + 1);
                Ok(MicFacts {
                    name: "mic".into(),
                    rate: 24_000,
                    channels: 1,
                })
            })
            .unwrap();
        assert_eq!(probes.get(), 3);
    }
}

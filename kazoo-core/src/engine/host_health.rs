impl super::EngineHandle {
    pub fn check_audio(&self) -> crate::Result<()> {
        self.stats.check_audio()
    }
}

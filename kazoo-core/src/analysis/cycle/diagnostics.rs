use std::cell::Cell;

thread_local! {
    static ENABLED: Cell<bool> = const { Cell::new(false) };
}

pub fn enabled() -> bool {
    ENABLED.get()
}

pub fn enable(value: bool) {
    ENABLED.set(value);
}

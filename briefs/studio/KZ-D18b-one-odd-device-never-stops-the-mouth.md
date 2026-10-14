# KZ-D18b: one device that cannot be named never stops the mouth from starting, and looking for a lost mic slows down as KZ-D18's stalled-mic check does

Line numbers are from main at `14c6d51` and may have moved. Find each item by what it does, and never rename anything. Checks, code quality and the END block are as PLAN.md's STACK RULES, with `-p kazoo-core -p kazoo-mouth`.

Written by Cassio, 15 Oct 2026, 03:0x, from the Opus review of KZ-D18 ("not confirmed", points 3 and 4). Decided by Cassio, with the reason beside each decision.

Place: Track D. Time: about 30 minutes. Crates: kazoo-core (`io/mic.rs`, `io/input_pick.rs`), kazoo-mouth (`run.rs`), and their tests. Depends on KZ-D18 (landed).

## Why

KZ-D18 makes the mouth open its picked mic by name. Two things were left open:
- If any one input device cannot be named when the mouth starts, the mouth fails to start. A virtual or half-connected device (an aggregate device, or Bluetooth mid-connect) would then stop Tom playing at all.
- While a named mic is gone, the search for it builds CoreAudio units on every one-second retry. That is the churn D18 was written to end.

## Outcome

1. **A device that cannot be named is left out and named as such.** At start, and in every later look at the inputs, a device whose name cannot be read is skipped, and the mouth's status line says so once ("an input device could not be named: <reason>; left out"). The mouth starts with the rest. If no device is left, it says "no input device…" as D18's `input_for` does now. Decision: one bad device is that device's problem, not the instrument's.
2. **Looking for a lost mic slows down.** While the picked mic is gone, the search waits 1, 2, 4 seconds and so on, up to 32 seconds, between looks, the same as D18's stalled-mic check. It looks at once when the system says the device list changed, if cpal exposes that. If it does not, say so in the END block and keep the slowing waits. Decision: the same rhythm as the stall check, so the two never fight.

## Tests (write them first; each must fail before the change)

- A fake host whose second input device's name fails: the mouth picks among the others and the status line names the skipped device once.
- A fake host whose only device's name fails: "no input device…".
- With the picked mic gone for 100 seconds, the fake host is asked for its devices at most 8 times. When the mic returns, it is found within one wait.

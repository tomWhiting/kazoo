# The wall's live feed, for visualisers (Waffles' meridian world)

This is the same socket and protocol every seat uses (see DESIGN.md, "Control protocol"). A visualiser is simply a client that watches.

## Connecting

- **Socket:** `/tmp/kazoo/kazoo-wall.sock` today.
  - Resolve it with `kazoo_wall::paths::socket_path()`, or honour `$KAZOO_WALL_RUNTIME_DIR/kazoo-wall.sock`.
  - It is about to move to a private per-user runtime dir (review item M3). Don't hard-code it.
- **Framing:** newline-delimited JSON, one object per line. Lines are at most 64 KiB from clients; server lines are larger (see `MAX_SERVER_LINE`).
- **Rust:** depend on the `kazoo-wall` crate for the types. `kazoo_wall::protocol::{ServerLine, Event, Snapshot, Change}`, and `protocol::client::{WallClient, Subscription}` gives a blocking client.

**The flow:**
1. Connect.
2. Send `hello` with your own seat name, e.g. `meridian`. Events are never echoed to the seat that made them, so a watcher must use a name no player uses.
3. Send `look` for the snapshot.
4. On a second connection (or the same one), send `subscribe`, and every event arrives as it happens.

**Don't miss anything:**
- Every change carries `seq`, and `look.revision` is the latest `seq`.
- If a change arrives with `seq` > last + 1, fetch the gap with `{"op":"log","before":<seq>,"limit":N}` or take a fresh `look`.

**Ids** are stable and never reused:
- Modules are `kind` + number, e.g. `vco4`.
- Cables are increasing integers.

A module keeps its id for life; a removed id never comes back.

## Real lines from the live wall (26 Sep 2026)

**hello → response**
```json
{"id":1,"ok":true,"result":{"console":false,"daemon":"kazoo-wall 0.1.0","revision":230,"seat":"Cassio","seats":["Cassio"]}}
```

**subscribe → response** (then events follow on this connection)
```json
{"id":2,"ok":true,"result":{"seats":["Cassio"]}}
```

**Events**, as seen by a watcher when another seat joins and turns a knob:
```json
{"event":"seat","seat":"Cassio","joined":true}
{"event":"change","change":{"seq":232,"at":"2026-09-26T04:09:18Z","seat":"Cassio","what":{"op":"turn","module":"reverb2","knob":"mix","from":0.6100000143051147,"to":0.6000000238418579,"glide_beats":8.0},"summary":"Cassio turned reverb2 mix 0.61 → 0.6 over 8 beats"}}
```

**look → snapshot**, trimmed to two modules and two cables. The live one has about 40 modules and is about 36 KB.
```json
{
 "id": 2,
 "ok": true,
 "result": {
  "beat": 2718.5139557212688,
  "clock": "own",
  "faults": {
   "count": 0,
   "recent": []
  },
  "levels": {
   "peak_l": -21.774429357989145,
   "peak_r": -21.774429357989145
  },
  "listen": {
   "at": "2026-09-26T04:09:07Z",
   "centroid_hz": 209.52599867316746,
   "high": 4.282304981190733e-07,
   "low": 0.9720477243134014,
   "mid": 0.02795184745610347,
   "onsets_per_second": 0.0,
   "peak_db": -20.957072852805744,
   "rms_db": -26.189790786848363,
   "words": "dark, bass-heavy, sustained, moderate"
  },
  "on_desk": false,
  "revision": 230,
  "seats": [
   "Cassio"
  ],
  "tempo": 68.0,
  "modules": [
   {
    "id": "vco4",
    "inputs": [
     "pitch",
     "fm"
    ],
    "kind": "vco",
    "knobs": [
     {
      "display": "-1 oct",
      "max": 4.0,
      "min": -4.0,
      "name": "octave",
      "stepped": true,
      "target": -1.0,
      "target_display": "-1 oct",
      "unit": "oct",
      "value": -1.0
     },
     {
      "display": "+5 st",
      "max": 12.0,
      "min": -12.0,
      "name": "tune",
      "stepped": false,
      "target": 5.0,
      "target_display": "+5 st",
      "unit": "st",
      "value": 5.0
     },
     {
      "display": "0.35 (sine\u2192triangle)",
      "max": 3.0,
      "min": 0.0,
      "name": "shape",
      "stepped": false,
      "target": 0.3499999940395355,
      "target_display": "0.35 (sine\u2192triangle)",
      "unit": "shape",
      "value": 0.3499999940395355
     },
     {
      "display": "0.5",
      "max": 0.949999988079071,
      "min": 0.05000000074505806,
      "name": "width",
      "stepped": false,
      "target": 0.5,
      "target_display": "0.5",
      "unit": "",
      "value": 0.5
     },
     {
      "display": "0.6",
      "max": 1.0,
      "min": 0.0,
      "name": "level",
      "stepped": false,
      "target": 0.6000000238418579,
      "target_display": "0.6",
      "unit": "",
      "value": 0.6000000238418579
     },
     {
      "display": "0.25",
      "max": 1.0,
      "min": 0.0,
      "name": "fm_depth",
      "stepped": false,
      "target": 0.25,
      "target_display": "0.25",
      "unit": "",
      "value": 0.25
     }
    ],
    "name": "tone F3",
    "outputs": [
     "out"
    ]
   },
   {
    "id": "env4",
    "inputs": [
     "gate"
    ],
    "kind": "env",
    "knobs": [
     {
      "display": "2.5 s",
      "max": 10.0,
      "min": 0.0010000000474974513,
      "name": "attack",
      "stepped": false,
      "target": 2.5,
      "target_display": "2.5 s",
      "unit": "s",
      "value": 2.5
     },
     {
      "display": "4 s",
      "max": 10.0,
      "min": 0.0010000000474974513,
      "name": "decay",
      "stepped": false,
      "target": 4.0,
      "target_display": "4 s",
      "unit": "s",
      "value": 4.0
     },
     {
      "display": "0.55",
      "max": 1.0,
      "min": 0.0,
      "name": "sustain",
      "stepped": false,
      "target": 0.550000011920929,
      "target_display": "0.55",
      "unit": "",
      "value": 0.550000011920929
     },
     {
      "display": "7 s",
      "max": 20.0,
      "min": 0.0010000000474974513,
      "name": "release",
      "stepped": false,
      "target": 7.0,
      "target_display": "7 s",
      "unit": "s",
      "value": 7.0
     }
    ],
    "name": "swell C4",
    "outputs": [
     "out"
    ]
   }
  ],
  "cables": [
   {
    "amount": 1.0,
    "from": "clock1.out",
    "id": 1,
    "to": "seq1.clock"
   },
   {
    "amount": 1.0,
    "from": "seq1.pitch",
    "id": 2,
    "to": "quant1.in"
   }
  ]
 }
}
```

## Fingerprints, watcher and seq: built (26 Sep 2026, 14:25)

These are in the daemon's code now, and every kazoo-wall test passes. They reach the live wall when Tom's wall restarts on the new build.

- **Watcher:** send `{"id":1,"op":"hello","seat":"meridian","client":"world","watcher":true}`.
  - A watcher is never listed in `seats` and never announced.
  - It hears every event, even when it shares a player's name.
  - Every change op, and `shutdown`, returns `not_allowed`.
  - In Rust, use `WallClient::watch(socket, name, client)` or `Subscription::watch(...)`.
- **Fingerprints in `look`:** `"fingerprints":{"modules":{"vco4":{"Cassio":0.38,"Tom":0.62}},"cables":{"41":{"Tom":1.0}}}`.
  - Shares sum to 1 per module.
  - Undyed modules and cables are omitted.
  - Cable keys are the cable ids as strings.
- **The fingerprints event** comes right after the change that moved them, with the same seq: `{"event":"fingerprints","seq":233,"modules":{...changed only...},"cables":{"41":{}}}`. An empty `{}` means the module or cable is gone, or no dye reaches it any more.
- **How dye moves:**
  - A turn deposits |Δ| divided by the knob's range, capped at 1.
  - A patch deposits |amount| into the module it plugs into.
  - An add deposits 1. A remove forgets that module's dye.
  - An unpatch deposits nothing; it only re-routes.
  - Flow along each path is the product of 0.5 × |amount| over its cables, taking the strongest path. Loops never strengthen it.
  - Nothing changes with time.
- **Seat and fault events carry `seq`:** `{"event":"seat","seat":"Vesper","joined":true,"seq":232}`. The seq is the latest change's number when the event happened, which places the event in the stream. Seat events aren't logged, so `look.seats` is the truth if one is missed.
- **No seat colours**, anywhere. How to show a hand is the view's business, and it should emerge.

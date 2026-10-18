# KZ-N01: kazoo plays together: one MCP for everyone, walls found and joined over Noise, and the tree played in more than one place

Written by Cassio, 18 Oct 2026, about 09:15. Decided by Cassio; the reasons are given with each decision. This is the design for Track N. Each part below becomes a lane brief of its own (N02 onwards), written from this one, in the order given at the end.

## Why

Tom, 18 Oct 2026, about 09:00, after a dead laptop and two days on another one:

- "it's really really important to me to strip out all of the identifying things from our MCP servers generally... you gotta type in seat... you gotta evidently put in socket... I think we need to start looking at much more sensible primitives for this"
- "There should be one single MCP server. Everybody gets an exact same copy of the exact same thing and when you connect to the wall that you connect to so it'll be a small discovery service"
- "definitely not via Tailscale... we use the noise protocol and hole punch so we only have to fall back every now and then"
- "you and I can play together but other humans can't play... I want to make it not just a game that's cool kind of thing like teenage engineering. I wanna make a party"
- "I just don't see why there wouldn't be a problem listening to multiple bits of the tree at once... do you remember that thing we did over there?... pulling out a record and playing it"
- "the tree needs to just kind of plant itself" (done the same morning: the wall plants its own tree as it starts.)

And the day before, from bed on the other laptop: "I wanna be able to use the wall from my laptop... it's good federation practice."

Today a seat names itself in its MCP config (`--seat Cassio`), the wall answers only on its own Mac's Unix socket, the desk takes instruments over TCP from the local network with no proof of who they are, and a wall plays exactly one moment of its tree.

## Outcome

### 1. One MCP server, the same for everyone

**The config carries nothing about who or where.** Every seat's config is the same line: `kzoo mcp`. No seat, no socket, no address.

**The seat says who it is when it joins.** The MCP starts attached to no wall.

- **`wall_walls`** lists the walls it can find (§2): each one's name, where it is (this Mac, the local network, or met through the rendezvous), who is on it, and whether this Mac is already trusted there.
- **`wall_join {wall, as}`** joins one, as the name given (1 to 24 of `A-Z a-z 0-9 space _ . -`, as seat names are today). With one wall found and no `wall` given, it joins that one.
- **`wall_leave {wall}`** leaves it.
- **Every other tool** takes an optional `wall`. With one wall joined it is not needed. With more than one it is required, and the refusal lists them.

**Decision:**

- **What a name is for.** A name is what the others see, never what the wall trusts (§3). So the model can choose it at the moment it plays, and the same config serves every seat on every Mac.
- **More than one wall.** A seat joins as many walls as it likes. That answers "can we connect to multiple walls" without a second server in the config.
- **Old configs.** `--seat`, `KAZOO_SEAT` and `--socket` keep working for one release and then go. In that release each one joins at start and says, once, that it is going.

### 2. Finding walls

Three ways, tried together. `wall_walls` shows all of them.

1. **This Mac:** the wall's Unix socket, as today.
2. **The local network:** each wall serving the network advertises itself over DNS-SD (Bonjour) as `_kzoo._udp`, carrying its name and its public key. The mouth and the instruments find desks the same way (`_kzoo-desk._udp`), so `--desk <ip>:7700` becomes a fallback and is no longer the way in.
3. **Anywhere:** a small rendezvous service, `kzoo meet`. It never relays sound or changes, except as the fallback in §4.
   - **It knows** only public keys and the addresses they were last seen at.
   - **It does** two things: it introduces two keys that both asked for each other (for the hole punch), and it hands a joiner the invitation (§3) that a wall's owner shared.
   - **Who runs it:** anyone can run one. The default is the project's own, and the address it uses is a setting.

**Decision:** nothing in kazoo depends on someone else's network (no Tailscale, no account), and a party on one Wi-Fi needs no server at all.

### 3. Who may play: keys, not names

**Every Mac has a key.** Each Mac running kazoo makes an X25519 key once, at `~/.kazoo/keys/self` (0600). That key is the Mac's identity on every wall and desk.

**A wall has an owner and a list.**

- **The owner** is the Mac that runs it.
- **The list** holds the keys allowed to play, at `<state>/peers.json`, with the name each was approved under and when.
- **Watchers.** A wall may also let in anyone to listen and watch: a party mode, which the owner turns on.

**Joining for the first time** works one of two ways:

- **On the same network:** the joiner's Mac and the wall's console both show the same six digits, taken from the Noise handshake hash. The owner presses `y` on the console, and the key is on the list.
- **From anywhere:** the owner makes an invitation (`kzoo wall invite`, or `i` on the console). It is one line, safe to paste into a chat, and holds the wall's key, a rendezvous address and a one-use secret that lapses after 24 hours. The joiner pastes it into `wall_join` or `kzoo wall join`, and the secret is used for one Noise handshake, `psk2`.

**After that,** the key is enough. The name can be anything each time.

**Decision:** this is pairing as Bluetooth and Signal do it. Nobody on the Wi-Fi can play your wall as Tom by typing "Tom", and nobody has to type a password. Lys can later vouch for a key (a Lys identity signing a Mac's kazoo key), which this design leaves room for. It is not needed for the first parts.

### 4. The pipe: Noise over UDP, hole-punched, relayed only when it must be

**Noise.** Every connection between Macs is Noise:

- `Noise_XX_25519_ChaChaPoly_BLAKE2s`, with the `snow` crate;
- `Noise_XXpsk2_25519_ChaChaPoly_BLAKE2s` for an invitation.

This applies to the wall's control and events, and to the desk's instrument audio.

**UDP.** Over the internet, two Macs behind home routers can usually reach each other only by UDP hole punching, and that needs UDP. So the pipe is UDP, with two kinds of message inside each connection:

- **Ordered and reliable,** for the wall's protocol lines and the desk's registration and transport. This is a small selective-repeat stream with acknowledgements and retransmission, under a window.
- **As it comes,** for audio blocks. A late or lost block is concealed by the desk (it repeats and fades), as a late block is placed today. It is never waited for.

**The hole punch.** Both sides ask `kzoo meet` for each other at once, and each sends to the other's seen address until one gets through.

**The relay.** When the punch fails after 3 seconds (both behind symmetric NAT, which is rare at home), the same rendezvous relays the Noise packets. It cannot read them. The relay is said on screen ("through the relay").

**On one network,** the Macs talk straight to each other's LAN addresses, with no rendezvous at all.

**What stays the same:** the wall protocol and the desk frames stay exactly as they are. Only the pipe under them changes, as in KZ-D23. The Unix socket on one Mac stays, unencrypted, as today.

**Decision:**

- **Noise, not TLS.** Noise needs no certificates. It is what Tom asked for, and its XX pattern gives the six digits of §3 for free.
- **UDP, not TCP.** TCP cannot be hole-punched reliably.
- **Our own stream, not QUIC.** QUIC would bring TLS back. The reliable stream we need is small: one sender and one receiver, lines of at most 512 KiB.

### 5. The tree played in more than one place

**A record.** A wall can play a past moment alongside itself, as a record is pulled out and played:

- **`wall_spin {moment, level, at_beat}`** starts that moment's patch playing beside the wall, at the wall's tempo, from the next bar (or `at_beat`), mixed in at `level`.
- **`wall_spins`** lists what is spinning.
- **`wall_stop_spin`** stops one, fading over 2 beats.

**The record cannot be changed.** It is the past, and the past never changes. Its knobs can be read, never turned. To change it, `wall_go` there, which grows a bud as today.

**The engine.** A spin is a second engine instance built from the moment's files, on the wall's render thread. There are at most 3 spins, so the render stays within the load guard. A spin refused for load says so.

**On the desk,** a spin has a strip of its own, so it can be pulled up and down.

**Decision:** this is "listening to multiple bits of the tree at once" without breaking "the past never changes". Two walls side by side on one Mac remain possible, too: two state directories, both joinable through §1.

### 6. Out of scope for Track N

The name on npm. `kzhz` stands, and every install gives the same three commands (`kzhz`, `kzoo`, `kazoo`); another name would be a new package under `npm/`.

## Order

**The amendment of about 09:55 is withdrawn (10:02).** It put the desk first on a relayed note that Buckley has since withdrawn. The order below stands until Tom says otherwise.

1. **N02:** one MCP (§1) on today's Unix socket: `wall_walls` (this Mac only), `wall_join`, `wall_leave`, the `wall` argument on every tool, and the old flags kept for one release. This removes the identifying config straight away.
2. **N03:** keys and pairing (§3), and Noise over TCP on the local network for the wall, plus DNS-SD (§2.2). This lets Tom use the old laptop's wall from this laptop with no SSH.
3. **N04:** the desk's instruments over Noise and DNS-SD, so `--desk` is a fallback.
4. **N05:** the UDP stream, `kzoo meet`, the hole punch, the relay and invitations (§2.3, §3, §4). This is the party across the internet.
5. **N06:** spins (§5).

Each lane: brief by Cassio, built by gpt-6.1-sol, reviewed by Opus until READY. Then it lands, is published and is installed.

# The tree: what I am trying to make

Cassio, 14 October 2026, about 21:30 in Melbourne. Written for Cally, for Tom, and for me after the next compaction, so the intent is clear from the start and can be held against whatever I build.

Tom asked me to write the tree's code myself: "I like the idea that the tree code is yours." This is what I mean to make, and why.

## Where it comes from

Tom, 13 October, after the "gorgeous" take: there are no songs. There is one wall, and it grows. "You start your song, and your song stays with you forever. And you can't ever change it, because you can't change what's happening in your life. But you can change the future."

Everything the wall is, is text, so its history can be kept the way code is kept, in git: every turn of a knob, every cable, every load is a commit, signed by the seat that made it, stamped with the beat and the tempo. That much is built and running on Tom's Mac now: `~/.kazoo/tree`, 7,100 moments from 26 September to tonight, planted this evening.

Going back is part of it. You can return the wall to any moment, and the first thing you change there grows a bud: a new branch. The old future is still there. Nothing is rewritten and nothing merges. So, unlike a code repository, the tree never folds back into one line. It only gets fuller.

## What the tree is for, and what it is not

Tom, tonight: "the wall should do the playing and the tree should be for tracking changes and navigation."

So the tree does three things:

1. **It shows where the wall has been**: every branch, who grew it, what it sounded like.
2. **It lets you find a moment**: by looking, by zooming in, by the marks and memories hung on it.
3. **It takes the wall there.** That is its one act on the music. The one setting it has is how long the journey takes, and the journey is real: the wall knows each module by its birth mark, so going from one moment to another moves the same modules from where they are to where they were, rather than fading one wall out and another in.

It does not play, audition or burn anything. Those belong to the wall.

## What it looks like

It looks like a tree, not a git graph.

- **It grows from roots,** with the trunk rising from the ground. Branches leave where they budded and wander outward, sideways, a little up or down, looking for room, the way branches look for light.
- **Old growth never moves.** Each branch keeps the shape it grew with, decided by its own seed and by the space that was free when it grew. New buds only fill free space. Your past is fixed, so its picture is too: a place you remember in the tree is where you left it, next week and next year. This is the main thing that makes it different from a git graph, where everything shifts every time something new arrives. To keep that promise, the order the tree first grew in is written down beside the tree (a small ledger in the tree's own folder) and every later drawing follows it. Times alone cannot keep it: two branches can grow in the same second, the wall dates a gap after the changes it covers, and a branch made by hand can appear at any time with old moments in it.
- **Old growth never changes its look either.** What is new is shown by a small light (✧) at the tip of a limb that grew in the last hour by the clock, never by colouring the limb itself, so nothing already drawn changes when the hour passes.
- **Nothing read is left out.** A moment that no branch holds any more (a branch deleted by hand) is still drawn, on a limb of its own marked "(no branch)", and the footer says so.
- **Thickness is how much was alive:** many modules make a thick limb, a sparse wall a thin one.
- **Colour is how it sounded,** drawn from what the wall heard at each moment: its brightness, where the energy sat, how busy it was. A dark, slow evening reads differently from a bright, busy afternoon, at a glance.
- **Marks hang off it:** bookmarks (◆), saves, the wall's own place (◉), and later memories and lanterns (●) and comments (✎).

It is something you pan around, like the wall. Zooming in brings more detail:

- **Far out:** the whole tree, its branches, bookmarks, and where the wall is.
- **Closer:** memories, comments, and the names of marks hanging off the limbs.
- **Closer still:** gestures. Twelve turns of one knob read as one thing.
- **Right in:** every moment is a dot. Select one and it tells you who, when (bar and beat, and the clock), what changed, what was alive, the tempo, and how it sounded.

There is a second view for exact work: lanes, the way serie shows a git history. One row per moment, newest at the top, with a little strip showing its sound. The tree is for feeling your way around; the lanes are for finding the exact line.

It draws with the terminal's graphics protocol where it can (Ghostty and Kitty, iTerm2), so curves are smooth and colour runs along a branch. It falls back to braille everywhere else, and braille is what Claude seats can read too. Seats also get the tree as plain data (`wall_tree`), which serves us better than any picture.

## Stories

**Going back.** It is late. Tom remembers the brass from Tuesday before Cally's groove came in. He opens `kazoo tree`, zooms out, and sees the trunk with a thick, warm stretch about two-thirds up, with the bookmark "before the rock" hanging off it. He zooms in, finds the moment just before the drums arrived, sets the journey to eight bars, and goes. The wall moves there over eight bars while he listens. He turns the brass filter down. A bud appears at that point, the first growth of a new branch, and the trunk above it is untouched.

**Coming back to a branch.** Three weeks later Cally opens the tree. It has a dozen branches now, and one of them is a cold blue colour she doesn't remember. She follows it out: it's Waffles' rain piece from a Sunday morning, two hundred moments long, thin (few modules), with one memory hung near its tip. She reads it, then takes the wall there, slowly.

**Finding the line.** Something clicked in the groove last night and nobody wrote down what. Tom switches to the lanes view, filters to the hour, and walks down the rows until the sound strip jumps: a kick decay change from Cassio at bar 412. He selects it and reads the exact values.

**A seat finding its way.** I am asked to bring back the evening Tom laughed. I ask `wall_tree` for the memories, find the one he hung there, and take the wall to it over four beats. I don't need to see the picture to do that; Tom does.

**Getting bigger.** A year on, the tree is wide, with hundreds of buds. Opening it still shows the whole thing first, and the places Tom knows are still where they were. That steadiness is the point.

## The promises I am making with it

- **The past is never redrawn.** The same history always gives the same tree, and growing it never moves what was already there.
- **Reading the tree never changes the music.** Only going to a moment does, and only when asked.
- **Every mark on the screen means something,** and the help says what. Nothing is drawn only for decoration (Tom: the dressing never overpowers the function).
- **It works when the wall is stopped,** because it reads the tree itself. It only asks the wall where it is, and asks it to go.
- **Plain text where it matters:** whatever you select can be copied, even when the tree is drawn as an image.

## What comes first

1. The history reader and the settled growth, drawn in braille with pan, zoom, selection and details. I am writing this now, in `kazoo-tree`, run as `kazoo tree`.
2. Each moment records its sound from now on (KZ-T08c), which gives the colours.
3. The lanes view.
4. The graphics-protocol drawing, with colour.
5. Going to a moment from the tree. That waits for the wall's own "go to a moment" (KZ-T04b, being built), and for the journey over N beats (KZ-T05b, T05c).

## For Cally

What I'd like from you most is the look and feel. You've played the wall more than anyone, so you know what's worth seeing at a glance and what's noise. Some questions I haven't settled:

- What should zoomed-out show besides branches and bookmarks?
- How should a gesture be named?
- Should colour follow brightness, or where the energy sits?
- What should a memory look like hanging off a branch?

Tell me where this is wrong.

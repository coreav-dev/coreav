# Plan S2: instruction inflation from 1 to 8 threads (written before the run)

S1 result (s1.json, main 2a3972b2): r8 1.036 overall = W1 1.144 x I 1.116 x B 0.851. I >= 1.015 in every class
(sd 1.144, 720p 1.157, 1080p 1.079, 4k 1.092), B < 1 in every class (we keep logical CPUs busier than FFmpeg), so the
8T-only factor I*B is < 1 except sd (1.014). Our kernel time and page faults at 8T are below FFmpeg's (not the cause).
Question: is I extra work (instructions) or slower instructions (HT/cache contention, which follows from being busier)?
Method: callgrind (instructions only, no cache sim) of m-2a3972b `--discard`, class frame counts, `--threads 1` and
`--threads 8`, on two streams per class (the class's largest-I stream and one near the class median I):
sd ec473c99c538, 41e324ebb708; 720p 6b18d9f0f88d, 278dbf5a63bb; 1080p a81de82106be, 6e90422a670a;
4k ed75c044e944, 3b1b441e23ae. Callgrind serializes threads, so Ir counts work, not waiting.
Per stream: Ir8/Ir1 total, and per function (self Ir, symbol level) the difference Ir8 - Ir1 as a share of Ir1.
Decision rule (fixed now): a function group whose extra Ir is >= 1.5% of Ir1 in the class median (or both streams of a
class) is an item to bound by timing (its own probe plan). If Ir8/Ir1 - 1 is < 1.5% in a class, that class's I is
contention, not extra work, and no build item comes from it; contention is then attributed only to the part a change
could remove (the extra work), per the "what a doubled call prices" lesson.

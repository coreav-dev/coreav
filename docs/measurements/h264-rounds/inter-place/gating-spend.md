# Spend of the inter placement round's Compute Engine runs

By the bound (hours the VM existed times 1.20 USD an hour): IP-2 3.152 USD (`gating-gcp-ip2/`); IP-6 first run
4.108 USD (void: the in-call gate defect fixed in an earlier change, and the 3.5 h limit; its files on branch
`measured/h264-inter-place-gating`, `gating-gcp-ip6-void1/`); IP-6 second run 3.182 USD (`gating-gcp-ip6/`).
**10.442 USD** for the round's eight-thread arms. Each run had its own cap in the harness (`--cap-usd`); the round's
total is above the 10 USD per-task limit, which the maintainer raised to 12.5 USD for this round after the void
run. The one-thread arms were not run.

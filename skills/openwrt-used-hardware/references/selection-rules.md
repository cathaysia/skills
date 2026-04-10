# OpenWrt Used Hardware Selection Rules

## Core Inputs

- `openwrt.json`: OpenWrt-supported device/profile list
- `toh.json`: local Table of Hardware export with `entries`, `columns`, and captions
- Xianyu / Goofish live listings for current market prices

## TOH Mapping

Convert `toh.json` rows into dict-like records by zipping `columns` with each row in `entries`.

Useful TOH fields:

- `deviceid`
- `brand`
- `model`
- `devicetype`
- `rammb`
- `flashmb`
- `ethernet100mports`
- `ethernet1gports`
- `ethernet2_5gports`
- `ethernet5gports`
- `ethernet10gports`
- `sataports`
- firmware URL fields

Compute port count from the Ethernet port columns above. Treat `-`, empty strings, and `None` as zero.

## Storage Interpretation

Use a conservative rule when the user asks for onboard storage:

- Strong pass:
  explicit `eMMC`
  explicit onboard size above threshold such as `8`, `16`, `32`, `64`
  obvious built-in SATA / NAS disk capability when the user allows disk-backed devices

- Weak evidence only:
  plain NAND values without clear usable onboard capacity
  microSD-only devices
  listings that mention expansion but not included storage

Do not over-claim storage from ambiguous TOH strings.

## Price Collection

Use Xianyu as the live source for price discovery.

Search patterns:

- exact model
- brand + model
- common Chinese alias
- chip + model only when exact search is sparse

Track at least:

- minimum observed price
- median or typical price
- observed sample list

But do not let the minimum dominate the recommendation if the listing is obviously low-quality noise.

## Xianyu Noise Filters

Reject or heavily discount listings that are:

- remote flashing / support only
- firmware / system optimization only
- accessories, shells, heatsinks, power adapters
- broken / repair-only parts unless the user asked for them
- suspiciously underpriced with no matching machine details

Prefer listings that mention:

- RAM size
- eMMC / SSD / flash size
- port count or intended routing use
- full board / full machine / complete bundle

## Xianyu Session Rules

- Reuse the same `agent-browser --session-name ...` session after manual login.
- Cookie replay alone may be unreliable; the live session is safer.
- Re-snapshot immediately before click actions.
- Ref ids are page-local and not stable after reloads.

## "Add To Cart" Behavior

Xianyu web normally has:

- `聊一聊`
- `立即购买`
- `收藏`

It does not behave like a standard cart. If the user asks to add items to cart, use `收藏` and state that this is the closest web equivalent.

## Recommendation Heuristics

Use this order:

1. Must satisfy all hard constraints.
2. Must have a real, buyable listing.
3. Prefer clearer spec disclosure.
4. Prefer lower price at similar capability.
5. Prefer lower power and smaller footprint only after the above.

Typical tradeoffs from this workflow:

- `Sophos SG 105`: strongest pure value in low budget if generic x86 OpenWrt is acceptable
- `GL.iNet GL-MV1000`: lower power, smaller, cleaner appliance experience
- `NanoPi R3S 2+32G`: strong balance when its live price stays reasonable

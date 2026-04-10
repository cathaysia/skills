---
name: openwrt-used-hardware
description: Shortlist OpenWrt-supported used hardware, enrich it with TOH specs and live marketplace prices, and verify real buyable listings on Xianyu/Goofish. Use when Codex needs to screen routers, mini PCs, SBCs, NAS boxes, or firewall appliances for OpenWrt support, RAM/storage/port constraints, price ceilings, value ranking, or when the user wants matching items favorited on Xianyu.
---

# OpenWrt Used Hardware

Use this skill to turn a noisy OpenWrt hardware list into a buyable shortlist.

## Workflow

1. Load the local hardware sources first.
   Prefer a local `openwrt.json` for the OpenWrt profile list.
   Prefer a local `toh.json` for authoritative hardware metadata instead of re-scraping the Table of Hardware.

2. Build a strict candidate set before opening marketplaces.
   Match OpenWrt profile ids to TOH rows using firmware filenames, device ids, or other stable identifiers.
   Keep only entries that satisfy the user's hard constraints before doing price work.

3. Normalize the hardware facts.
   Extract or derive at least:
   `memory_mb`
   `flash_storage_raw`
   `emmc_present`
   Ethernet port count from TOH port columns
   Optional SATA / disk capability

4. Treat marketplace prices as noisy until verified.
   Do not trust the lowest observed price blindly.
   Exclude listings that are clearly only for flashing service, remote support, accessories, repair, shells, or configuration help.
   Prefer full-machine or full-board listings with explicit specs in title/body.

5. Use `agent-browser` with a persistent Xianyu session.
   Reuse the live logged-in session when possible.
   If the user logs in manually, keep using the same session name for follow-up work.
   Re-snapshot immediately before every click because refs are not stable across page loads.

6. Rank by value, not by absolute cheapest price.
   Favor listings that clearly satisfy RAM, onboard storage/eMMC, and port-count constraints.
   Prefer cleaner, more explicit listings over suspiciously cheap ambiguous ones.

7. When the user says "add to cart" on Xianyu, use `收藏`.
   Xianyu web pages typically expose `聊一聊`, `立即购买`, and `收藏`, not a normal cart.
   Explain briefly that favorites are the closest equivalent.

## Recommended Data Shape

When producing an enriched shortlist file such as `42.json`, keep each record compact and practical. Prefer:

```json
{
  "id": "qnap_301w",
  "brand": "QNAP",
  "model": "QHora-301W",
  "version": null,
  "memory_mb": 1024,
  "flash_storage_raw": ["8", "eMMC"],
  "emmc_present": true,
  "emmc_details": "8GB eMMC",
  "price_cny": {
    "min": 798,
    "median": 900,
    "observed": [798, 900, 999]
  },
  "queries": ["QHora-301W", "QNAP QHora-301W"]
}
```

## Browser Procedure

1. Open or reuse the Xianyu session.
2. Search with narrow model terms first.
3. Open a real listing page.
4. `snapshot -i --urls`
5. Find the current `收藏` ref.
6. Click it.
7. Re-snapshot and confirm the control changed to `已收藏`.

## Output Rules

State the hard constraints you applied.
Separate authoritative hardware facts from inferred or seller-provided details.
Call out when a device is supported through generic `x86/64` OpenWrt images instead of a device-specific firmware page.
When recommending a winner, explain whether it wins on price, power, ports, storage, or simplicity.

## Reference

Read [references/selection-rules.md](references/selection-rules.md) when you need the concrete filtering rules, anti-noise heuristics, TOH field mapping, and Xianyu operating notes from this workflow.

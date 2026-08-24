---
title: A polling cadence equal to the freshness budget has no jitter margin
date: 2026-08-24
last_updated: 2026-08-24
component: web-ui-server
problem_type: performance
---

# A polling cadence equal to the freshness budget has no jitter margin

## Problem

The Web monitor promised a 60-second p95 freshness budget and initially polled
every 60 seconds. A complete unselected 20-sample development sequence measured
60.0768 seconds p95, missing by 76.8 milliseconds even though ingestion itself
completed within milliseconds.

The full environment and every sample are recorded in
`specs/029-web-ui-server.md:859-971`.

## Root cause

A polling interval equal to the external freshness budget leaves no room for
timer scheduling, browser rendering, request latency, or DOM observation. Even
a healthy sequence can therefore exceed the promise while behaving exactly as
configured.

## Fix

`quill-j76f.19` reduced only `WEB_POLL_MS` from 60 seconds to 55 seconds. A new
complete 20-sample sequence measured 55.0560 seconds p95, leaving 4.9440 seconds
of margin. The fix landed as
`c6b8bf40b3f77bddd7f15430eaa5cddda3820566`.

## Prevention

- Set polling cadence below the user-visible freshness budget.
- Reserve explicit margin for scheduler, transport, render, and observation
  jitter instead of treating the interval as the whole budget.
- When a cadence changes, rerun a complete ordinary sequence; never reuse or
  cherry-pick samples from the old cadence.
- Record the producer timestamp and final observation timestamp so ingestion
  latency can be distinguished from polling delay.

---
lat:
  require-code-mention: true
---
# Pi Spool Drain Test Specs

These tests pin Quill's bounded replay of Pi tracking and runtime envelopes without racing live appenders or duplicating accepted events.

## Live overlap

A live-PID file remains appendable across drains; newly appended records land while repeated usage and runtime UUIDs remain deduplicated.

## Corrupt continuation and dead cleanup

A corrupt committed line records a typed gap without blocking later valid lines, and Quill deletes the dead writer's claimed file after the pass.

## Cap drop gap

File, directory, and age caps record a typed drop gap; dead files can be claimed and removed while live-PID files remain untouched.

## Ingest throttling

Four 15-second passes consume at most half of each Pi ingest window, leaving 2,000 requests per minute for live tracking and runtime traffic; throttled files retain their claimed remainder.

## Typed health gap

Provider health maps both corrupt-record and cap-drop gap codes to the typed spool error state exposed by integration status.

## Symlink boundary

The drain rejects a symlinked spool root and leaves every file in its external target untouched.

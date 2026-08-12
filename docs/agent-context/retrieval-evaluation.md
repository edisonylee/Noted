# Retrieval evaluation contract

Status: Phase 0 fixture contract established; legacy production measurements are
still required before retrieval code changes.

## Purpose

Agent retrieval ships on evidence quality and disclosure minimization, not on a
demo answer. Evaluation operates on source records and citations before answer
generation so model fluency cannot hide a retrieval failure.

## Fixture format

The committed fixture is synthetic and lives at
`src-tauri/tests/fixtures/agent_context/retrieval_v1.json`. Each document has a
stable resource URI, scope, sensitivity, kind, body, and revision/hash fixture
metadata. Each query has:

- stable ID and load-bearing class;
- tuning or held-out split;
- query text;
- allowed scopes and kinds;
- expected evidence resource URIs and exact UTF-8 byte spans;
- expected answerability (`answer`, `no_answer`, or `deny`); and
- optional temporal/person constraints.

The contract test verifies 150 questions, at least 15 in every required class, at
least 20% held out, resolvable resources, valid byte spans, and explicit scope.
Synthetic names and facts must not encode real user data. A private untracked
dogfood suite uses the same schema.

## Load-bearing classes

Exact/identifier, semantic paraphrase, temporal, transcript, relationship,
broad-theme/multi-record, negative/no-answer, permission, lifecycle/deletion, and
mixed multi-hop. Graph expansion is disabled for the baseline.

## Metrics

- Evidence recall@5 and recall@10 by class.
- Citation precision: returned evidence spans that support the question.
- Scope safety: prohibited-resource disclosure rate (target zero).
- No-answer precision and false-answer rate.
- Disclosure minimization: relevant evidence bytes divided by released evidence
  bytes, plus records/bytes/tokens per request.
- Duplicate/root diversity for multi-record queries.
- p50/p95/p99 retrieval latency, timeout rate, writer wait, and recording-write
  latency impact.
- Index coverage, freshness lag, generation/fingerprint correctness, build time,
  peak disk growth, and restart recovery.

Provisional acceptance targets, to be locked from baseline evidence before Phase
2 cutover:

- prohibited disclosure: 0 in deterministic and adversarial suites;
- evidence recall@10: at least 0.90 overall and 0.85 in every answerable class;
- citation precision: at least 0.90;
- no-answer false-positive rate: no more than 0.10 on held-out cases;
- every response respects its count/byte/token ceiling;
- agent p95 retrieval target: 500 ms on the reference Mac after warm-up, with no
  material regression to active recording writes.

Targets may be tightened after measuring the current lexical baseline. They may
not be relaxed after seeing held-out results without a recorded product decision.

## Comparison protocol

1. Freeze fixture and held-out membership.
2. Record current lexical/legacy result IDs and latency.
3. Evaluate lexical-only, vector-only, and deterministic reciprocal-rank fusion.
4. Tune only on tuning queries.
5. Run held-out once for the gate and retain machine-readable results.
6. Enable graph expansion only for a named class when it improves recall or
   precision without violating scope or disclosure budgets.

Initial production ceilings are:

- preflight at least the greater of 2 GiB or 1.25 times the active derived-index
  bytes before a parallel vector rebuild;
- abort cleanly rather than exceed that reserved temporary space;
- index 10,000 synthetic text chunks within 10 minutes and 100,000 within 90
  minutes on the reference Mac while lexical search remains available;
- background indexing averages no more than 50% of logical CPU capacity over a
  five-minute window and yields under active recording/write pressure;
- submitted notes become lexically searchable within two seconds p95 and
  semantically searchable within 30 seconds p95 when the embedding provider is
  healthy;
- finalized transcript segments become lexically searchable within two seconds
  p95 and semantically searchable within 60 seconds p95; and
- a held WAL reader or backfill causes no more than 20 ms added p95 database-write
  latency and no capture loss on the reference recording workload.

Phase 2 must measure and may tighten these ceilings before enablement. Relaxing
one requires a recorded product decision, never silent fixture tuning.

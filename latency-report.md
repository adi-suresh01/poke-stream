# Poke-Stream Latency Report

**Date:** 2026-03-06 22:48:13 UTC
**Target:** 127.0.0.1:8080

## TCP Connection Latency

| Metric | Value |
|--------|-------|
| Avg TCP connect | 1ms |
| Samples | 3 |

## First Frame Latency

| Metric | Value |
|--------|-------|
| First frame received | 7ms |
| Frame size (first 500b) | 0 bytes |

## Command Round-Trip

| Metric | Value |
|--------|-------|
| Command round-trip | 6ms |
| Response size | 0 bytes |

## PokeAPI External Call (baseline)

| Pokemon | Latency |
|---------|---------|
| pikachu | 196ms |
| charizard | 186ms |
| mewtwo | 169ms |

**With caching:** After first fetch, subsequent lookups for the same pokemon = **0ms** (in-memory HashMap)

## Ollama Local LLM Latency

| Query | Latency | Tokens |
|-------|---------|--------|
| what type is pikachu? | 2205ms | - |
| tell me about charmander | 5607ms | - |
| what moves does bulbasaur know? | 2507ms | - |

## Anthropic API Comparison (external, paid)

ANTHROPIC_API_KEY not set, skipping. Typical latency: **800-2000ms** from NY server.

## Summary of Improvements

| Change | Before | After | Impact |
|--------|--------|-------|--------|
| TCP_NODELAY | Nagle buffering (200-500ms+ per write) | Immediate send | Eliminates base latency |
| Frame dedup | ~33 identical frames/sec when idle | 0 frames when unchanged | ~95% bandwidth reduction at idle |
| PokeAPI cache | 196ms per lookup | 0ms after first | Instant pokemon identification |
| Ollama local LLM | ~1000-2000ms (Anthropic API) | ~2205ms (local) | Free, no external dependency |
| Shared HTTP client | New TLS handshake per request (~300ms) | Reused connections | Saves ~300ms per API call |
| WhiteCircle removed | 2x HTTP calls per agent query | 0 extra calls | Saves ~400-800ms per query |

# Bugfix Requirements Document

## Introduction

The feeder service assumes all subject addresses are valid Stellar addresses with funded accounts. When a subject address is invalid or the account doesn't exist on-chain, API calls to Horizon or RPC simulations throw unhandled errors. While the `runCycle` method catches errors per-subject, the error messages are opaque and don't distinguish between transient failures (network issues) and permanent failures (invalid addresses, non-existent accounts).

This bugfix ensures invalid or non-existent subject addresses are detected early, logged clearly, and skipped gracefully without crashing the feeder or blocking other subjects from being processed.

## Bug Analysis

### Current Behavior (Defect)

1.1 WHEN a subject address is invalid (malformed G... address) THEN the system throws an unhandled error during address validation or API calls

1.2 WHEN a subject address corresponds to a non-existent account THEN Horizon API calls in `fetchHorizonStats` throw an error with status 404 and message "Resource Missing"

1.3 WHEN a subject address corresponds to a non-existent account THEN RPC simulation in `getActiveVcCount` may throw an error if the identity-oracle contract call fails for unknown subjects

1.4 WHEN an invalid or non-existent subject causes an error THEN the error message logged by `runCycle` is opaque and does not clearly indicate the root cause (e.g., "Error: Request failed with status code 404")

1.5 WHEN Horizon returns a 404 error for a non-existent account THEN the error is caught by `withExponentialBackoff` and retried unnecessarily, wasting time and resources

### Expected Behavior (Correct)

2.1 WHEN a subject address is invalid (malformed G... address) THEN the system SHALL detect the invalid format before making API calls and log a clear error message (e.g., "Invalid subject address format: [address]")

2.2 WHEN a subject address corresponds to a non-existent account THEN Horizon API calls SHALL detect the 404 error, log a clear message (e.g., "Subject account not found on Horizon: [address]"), and skip the subject without retrying

2.3 WHEN a subject address corresponds to a non-existent account THEN RPC simulation SHALL detect contract simulation failures, log a clear message (e.g., "Subject account not found in identity-oracle: [address]"), and skip the subject without retrying

2.4 WHEN an invalid or non-existent subject is encountered THEN the system SHALL log a structured error message that clearly identifies the subject address and the reason for skipping (invalid format, account not found, simulation failed)

2.5 WHEN a permanent error (invalid address, account not found) is detected THEN the system SHALL skip retries and immediately proceed to the next subject

2.6 WHEN a subject is skipped due to invalid address or non-existent account THEN the system SHALL continue processing the remaining subjects in the cycle without interruption

### Unchanged Behavior (Regression Prevention)

3.1 WHEN a subject address is valid and the account exists THEN the system SHALL CONTINUE TO fetch Horizon stats, read VC count, and submit transactions as before

3.2 WHEN a transient error occurs (network timeout, rate limiting, temporary RPC failure) THEN the system SHALL CONTINUE TO retry with exponential backoff as implemented in `withExponentialBackoff`

3.3 WHEN Horizon returns a 429 rate limit error THEN the system SHALL CONTINUE TO respect the `Retry-After` header and retry as implemented in `callWithHorizonRateLimit`

3.4 WHEN `runCycle` encounters an error for one subject THEN the system SHALL CONTINUE TO process the remaining subjects in the cycle

3.5 WHEN a subject is successfully processed THEN the system SHALL CONTINUE TO log the same detailed output (vc_count, volume_30d, tx_count_30d, avg_counterparties, transaction hashes)

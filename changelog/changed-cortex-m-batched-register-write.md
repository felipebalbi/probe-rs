Writing a Cortex-M core register now takes one batched transaction instead of three, which speeds up every operation that calls into a RAM-based flash algorithm.

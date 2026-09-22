# OpenMayhem Core 0.2.231

- Retail TAP bridge retries preserve the first locked backing amount instead of repricing an already verified customer transfer before broadcast.
- Retail TNK bridge retries recover the existing canonical deposit-rate lock and reject any change to its locked transfer amount.
- TAP and TNK collection funding shortfalls are reported distinctly from network failures so operators can restore the affected rail directly.
- Contract version and contract code digest remain unchanged.

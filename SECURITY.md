# Security policy

Security fixes target the latest supported macOS release and the current `main` branch. Older builds may need to update before receiving a fix.

Please report a suspected vulnerability through [GitHub private vulnerability reporting](https://github.com/ultramenid/clipward/security/advisories/new). Do not include exploit details, provider keys, private videos, or diagnostic logs in a public issue. Include the affected version, steps to reproduce with dummy data, and expected impact. Maintainers will coordinate a private response and disclosure with you.

Clipward stores provider keys in the OS keychain, runs the clipping pipeline and Whisper transcription locally, and calls the AI provider the user picked (a logged-in CLI, OpenRouter, Anthropic or OpenAI) with the user's own account. Review [the data flow](docs/ARCHITECTURE.md) when assessing a report. If private reporting is unavailable, open a public issue requesting a private contact channel without vulnerability details.

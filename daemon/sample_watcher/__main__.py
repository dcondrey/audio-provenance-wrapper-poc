"""Entry point for `python -m daemon.sample_watcher`."""

from .watcher import main

if __name__ == "__main__":
    raise SystemExit(main())

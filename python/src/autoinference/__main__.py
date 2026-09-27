import sys


def main() -> int:
    print(
        "autoinference: the CLI is a Rust binary. Install it with one of:\n"
        "  cargo install autoinference\n"
        "  npm install -g autoinference\n"
        "  https://github.com/autoinference/autoinference/releases\n"
        "This Python package provides the sidecar (`python -m autoinference_sidecar`).",
        file=sys.stderr,
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())

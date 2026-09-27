"""autoinference — agentic LLM inference optimization.

The product is a Rust CLI (`cargo install autoinference`, `npm i -g autoinference`, or a GitHub
Release binary). This Python package installs the sidecar the CLI drives (knob registry, hardware
probes, engine adapters) and exposes its version.
"""

__version__ = "0.0.7"

try:  # the sidecar is the real Python surface
    from autoinference_sidecar import PROTOCOL_VERSION  # noqa: F401
except ImportError:  # pragma: no cover
    PROTOCOL_VERSION = None

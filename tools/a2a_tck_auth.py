"""Synthetic credential configuration for unmodified official A2A TCK tests.

Only the exact SUT loopback origin receives this non-secret fixture credential.
This plugin does not modify tests, expected values, payloads, or gateway policy.
"""

from __future__ import annotations

import ipaddress
import os
from importlib import import_module
from inspect import signature
from typing import ClassVar, Protocol, cast
from urllib.parse import urlsplit

import httpx
import pytest

FIXTURE_TOKEN = "vibemux_tck_fixture_token_01234567890123456789"
_patch = pytest.MonkeyPatch()


class TckGrpcClient(Protocol):
    """The small runtime API used from the TCK's untyped optional package."""

    _METADATA: ClassVar[tuple[tuple[str, str], ...]]

    def __init__(self, base_url: str) -> None: ...


def load_tck_grpc_client() -> type[TckGrpcClient]:
    """Validate the optional plugin API without modifying upstream type files."""
    module = import_module("tck.transport.grpc_client")
    candidate = getattr(module, "GrpcClient", None)
    initializer = getattr(candidate, "__init__", None)
    if not isinstance(candidate, type) or not callable(initializer):
        raise pytest.UsageError("The TCK gRPC client does not expose the expected class")
    metadata = getattr(candidate, "_METADATA", None)
    if not isinstance(metadata, tuple) or any(
        not isinstance(entry, tuple)
        or len(entry) != 2
        or not all(isinstance(item, str) for item in entry)
        for entry in metadata
    ):
        raise pytest.UsageError("The TCK gRPC client metadata contract is incompatible")
    try:
        signature(initializer).bind(None, "http://127.0.0.1:1")
    except TypeError as error:
        raise pytest.UsageError("The TCK gRPC client constructor is incompatible") from error
    return cast(type[TckGrpcClient], candidate)


class HttpSender[**Parameters](Protocol):
    """A callback preserving HTTPX positional and keyword argument names."""

    def __call__(
        _protocol_self,
        self: httpx.Client,
        request: httpx.Request,
        *args: Parameters.args,
        **kwargs: Parameters.kwargs,
    ) -> httpx.Response: ...


def wrap_http_sender[**Parameters](
    original_send: HttpSender[Parameters],
    allowed_origin: tuple[str, str, int],
) -> HttpSender[Parameters]:
    """Preserve the exact HTTPX call signature while adding fixture credentials."""

    def send_with_fixture_auth(
        self: httpx.Client,
        request: httpx.Request,
        *args: Parameters.args,
        **kwargs: Parameters.kwargs,
    ) -> httpx.Response:
        origin = (request.url.scheme, request.url.host, request.url.port or 80)
        if origin == allowed_origin and "authorization" not in request.headers:
            request.headers["authorization"] = f"Bearer {FIXTURE_TOKEN}"
        return original_send(self, request, *args, **kwargs)

    return send_with_fixture_auth


def pytest_configure(config: pytest.Config) -> None:
    sut_url = cast(str, config.getoption("--sut-host"))
    parsed = urlsplit(sut_url)
    if (
        parsed.scheme != "http"
        or not parsed.hostname
        or not ipaddress.ip_address(parsed.hostname).is_loopback
    ):
        raise pytest.UsageError("The VibeMux TCK auth adapter requires a numeric HTTP loopback SUT")
    allowed_origin = (parsed.scheme, parsed.hostname, parsed.port or 80)
    _patch.setattr(httpx.Client, "send", wrap_http_sender(httpx.Client.send, allowed_origin))
    grpc_client = load_tck_grpc_client()
    grpc_origin = os.environ.get("VIBEMUX_TCK_GRPC_URL", "")
    original_init = grpc_client.__init__

    def init_at_fixture_endpoint(self: TckGrpcClient, base_url: str) -> None:
        if (base_url if "://" in base_url else "http://" + base_url).rstrip(
            "/"
        ) != grpc_origin.rstrip("/"):
            raise pytest.UsageError("The TCK gRPC origin does not match the owned fixture receipt")
        original_init(self, base_url)

    _patch.setattr(grpc_client, "__init__", init_at_fixture_endpoint)
    _patch.setattr(
        grpc_client,
        "_METADATA",
        grpc_client._METADATA + (("authorization", f"Bearer {FIXTURE_TOKEN}"),),
    )


def pytest_unconfigure(config: pytest.Config) -> None:
    _patch.undo()

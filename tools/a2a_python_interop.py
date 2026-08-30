"""Independent official Python SDK interop fixtures; not the official A2A ITK."""

from __future__ import annotations

import argparse
import asyncio
import hmac
import ipaddress
import json
import socket
import sys
import uuid
from collections.abc import AsyncGenerator, Callable
from typing import TypedDict, cast
from urllib.parse import urlsplit

import grpc
import httpx
import uvicorn
from a2a.client.card_resolver import A2ACardResolver
from a2a.client.client import ClientCallContext, ClientConfig
from a2a.client.client_factory import ClientFactory
from a2a.server.context import ServerCallContext
from a2a.server.request_handlers import GrpcHandler, RequestHandler
from a2a.server.request_handlers.grpc_handler import GrpcServerCallContextBuilder
from a2a.server.routes.agent_card_routes import create_agent_card_routes
from a2a.server.routes.jsonrpc_routes import create_jsonrpc_routes
from a2a.server.routes.rest_routes import create_rest_routes
from a2a.types import a2a_pb2 as pb
from a2a.types import a2a_pb2_grpc
from a2a.utils.errors import InvalidRequestError, TaskNotFoundError, UnsupportedOperationError
from google.protobuf.message import Message as ProtoMessage
from starlette.applications import Starlette

TOKEN = "vibemux_tck_fixture_token_01234567890123456789"


class BindingCheck(TypedDict):
    binding: str
    discovery: bool
    send_get_identity: bool
    cancel_terminal: bool


class InteropResult(TypedDict):
    direction: str
    sdk_version: str
    checks: list[BindingCheck]


# This generated SDK registration function has no annotations. Its two
# arguments are the official handler and asyncio server used by this fixture.
register_grpc_service: Callable[[GrpcHandler, grpc.aio.Server], None] = (
    a2a_pb2_grpc.add_A2AServiceServicer_to_server
)


def loopback_origin(value: str) -> str:
    if not isinstance(value, str) or len(value) > 2048:
        raise ValueError("invalid fixture endpoint")
    parsed = urlsplit(value if "://" in value else "http://" + value)
    if (
        parsed.scheme != "http"
        or not parsed.hostname
        or not ipaddress.ip_address(parsed.hostname).is_loopback
        or parsed.username
        or parsed.password
        or parsed.query
        or parsed.fragment
        or parsed.path not in ("", "/")
        or not parsed.port
    ):
        raise ValueError("fixture endpoints must be numeric HTTP loopback origins")
    return f"http://{parsed.netloc}"


def grpc_channel(target: str) -> grpc.aio.Channel:
    origin = loopback_origin(target)
    return grpc.aio.insecure_channel(
        urlsplit(origin).netloc,
        options=[
            ("grpc.max_receive_message_length", 65536),
            ("grpc.max_send_message_length", 65536),
        ],
    )


async def verify_python_client(url: str) -> InteropResult:
    url = loopback_origin(url)
    checks: list[BindingCheck] = []
    for binding in ["HTTP+JSON", "JSONRPC", "GRPC"]:
        async with httpx.AsyncClient(timeout=8, trust_env=False, follow_redirects=False) as http:
            card = await A2ACardResolver(http, url).get_agent_card()
            for interface in card.supported_interfaces:
                loopback_origin(interface.url)
            assert any(
                interface.protocol_binding == binding for interface in card.supported_interfaces
            )
            config = ClientConfig(
                streaming=False,
                polling=True,
                httpx_client=http,
                grpc_channel_factory=grpc_channel,
                supported_protocol_bindings=[binding],
                use_client_preference=True,
            )
            client = ClientFactory(config).create(card)
            context = ClientCallContext(
                timeout=8, service_parameters={"authorization": f"Bearer {TOKEN}"}
            )
            message = pb.Message(
                message_id="tck-input-required-" + uuid.uuid4().hex,
                context_id="python_" + uuid.uuid4().hex,
                role=pb.ROLE_USER,
                parts=[pb.Part(text="Independent Python SDK probe")],
            )
            async with client:
                events = [
                    event
                    async for event in client.send_message(
                        pb.SendMessageRequest(message=message), context=context
                    )
                ]
                assert events and events[-1].HasField("task")
                task = events[-1].task
                assert task.id and task.context_id == message.context_id
                fetched = await client.get_task(pb.GetTaskRequest(id=task.id), context=context)
                assert fetched.id == task.id and fetched.context_id == task.context_id
                canceled = await client.cancel_task(
                    pb.CancelTaskRequest(id=task.id), context=context
                )
                assert canceled.id == task.id and canceled.status.state == pb.TASK_STATE_CANCELED
                checked = await client.get_task(pb.GetTaskRequest(id=task.id), context=context)
                assert checked.status.state == pb.TASK_STATE_CANCELED
                checks.append(
                    {
                        "binding": binding,
                        "discovery": True,
                        "send_get_identity": True,
                        "cancel_terminal": True,
                    }
                )
    return {"direction": "official_python_sdk_to_vibemux", "sdk_version": "1.1.3", "checks": checks}


class ReferenceHandler(RequestHandler):
    """A bounded external test peer; never opens a VibeMux database."""

    _agent_card: pb.AgentCard

    def __init__(self) -> None:
        self.tasks: dict[str, pb.Task] = {}

    def authorize(self, context: ServerCallContext) -> None:
        # Authorization is an ASCII HTTP/gRPC metadata value in the SDK contexts.
        value = cast(str, context.state.get("headers", {}).get("authorization", ""))
        if not hmac.compare_digest(value, f"Bearer {TOKEN}"):
            raise InvalidRequestError("fixture authentication required")

    async def on_message_send(
        self, params: pb.SendMessageRequest, context: ServerCallContext
    ) -> pb.Task:
        self.authorize(context)
        if len(self.tasks) >= 32:
            raise InvalidRequestError("fixture capacity")
        task = pb.Task(
            id="python_" + uuid.uuid4().hex,
            context_id=params.message.context_id or params.message.message_id,
            status=pb.TaskStatus(state=pb.TASK_STATE_INPUT_REQUIRED),
        )
        self.tasks[task.id] = task
        return task

    async def on_get_task(self, params: pb.GetTaskRequest, context: ServerCallContext) -> pb.Task:
        self.authorize(context)
        if params.id not in self.tasks:
            raise TaskNotFoundError()
        return self.tasks[params.id]

    async def on_cancel_task(
        self, params: pb.CancelTaskRequest, context: ServerCallContext
    ) -> pb.Task:
        task = await self.on_get_task(pb.GetTaskRequest(id=params.id), context)
        task.status.state = pb.TASK_STATE_CANCELED
        return task

    async def on_list_tasks(
        self, params: pb.ListTasksRequest, context: ServerCallContext
    ) -> pb.ListTasksResponse:
        self.authorize(context)
        tasks = list(self.tasks.values())[: min(params.page_size or 32, 32)]
        return pb.ListTasksResponse(
            tasks=tasks, next_page_token="", page_size=len(tasks), total_size=len(self.tasks)
        )

    async def on_message_send_stream(
        self, params: pb.SendMessageRequest, context: ServerCallContext
    ) -> AsyncGenerator[pb.Task, None]:
        yield await self.on_message_send(params, context)

    async def on_subscribe_to_task(
        self, params: pb.SubscribeToTaskRequest, context: ServerCallContext
    ) -> AsyncGenerator[pb.Task, None]:
        yield await self.on_get_task(pb.GetTaskRequest(id=params.id), context)

    async def on_create_task_push_notification_config(
        self, params: pb.TaskPushNotificationConfig, context: ServerCallContext
    ) -> pb.TaskPushNotificationConfig:
        raise UnsupportedOperationError()

    async def on_get_task_push_notification_config(
        self, params: pb.GetTaskPushNotificationConfigRequest, context: ServerCallContext
    ) -> pb.TaskPushNotificationConfig:
        raise UnsupportedOperationError()

    async def on_list_task_push_notification_configs(
        self, params: pb.ListTaskPushNotificationConfigsRequest, context: ServerCallContext
    ) -> pb.ListTaskPushNotificationConfigsResponse:
        raise UnsupportedOperationError()

    async def on_delete_task_push_notification_config(
        self, params: pb.DeleteTaskPushNotificationConfigRequest, context: ServerCallContext
    ) -> None:
        raise UnsupportedOperationError()

    async def on_get_extended_agent_card(
        self, params: pb.GetExtendedAgentCardRequest, context: ServerCallContext
    ) -> pb.AgentCard:
        raise UnsupportedOperationError()


class ReferenceGrpcContext(GrpcServerCallContextBuilder):
    def build(
        self, context: grpc.aio.ServicerContext[ProtoMessage, ProtoMessage]
    ) -> ServerCallContext:
        return ServerCallContext(state={"headers": dict(context.invocation_metadata() or ())})


async def serve_python_reference() -> None:
    handler = ReferenceHandler()
    grpc_server = grpc.aio.server(
        maximum_concurrent_rpcs=16,
        options=[
            ("grpc.max_receive_message_length", 65536),
            ("grpc.max_send_message_length", 65536),
        ],
    )
    grpc_port = grpc_server.add_insecure_port("127.0.0.1:0")
    if not grpc_port:
        raise RuntimeError("grpc bind failed")
    register_grpc_service(GrpcHandler(handler, ReferenceGrpcContext()), grpc_server)
    http_socket = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
    http_socket.bind(("127.0.0.1", 0))
    http_socket.listen(16)
    http_url = f"http://127.0.0.1:{http_socket.getsockname()[1]}"
    grpc_url = f"http://127.0.0.1:{grpc_port}"
    card = pb.AgentCard(
        name="Official Python SDK reference",
        description="Owned local interoperability fixture",
        version="1.1.3",
        supported_interfaces=[
            pb.AgentInterface(protocol_binding="HTTP+JSON", url=http_url, protocol_version="1.0"),
            pb.AgentInterface(protocol_binding="JSONRPC", url=http_url, protocol_version="1.0"),
            pb.AgentInterface(
                protocol_binding="GRPC", url=f"127.0.0.1:{grpc_port}", protocol_version="1.0"
            ),
        ],
        capabilities=pb.AgentCapabilities(streaming=False, push_notifications=False),
        default_input_modes=["application/json", "text/plain"],
        default_output_modes=["application/json", "text/plain"],
        skills=[
            pb.AgentSkill(
                id="fixture",
                name="Fixture",
                description="Task identity and cancellation",
                tags=["fixture"],
            )
        ],
    )
    handler._agent_card = card
    routes = (
        create_agent_card_routes(card)
        + create_jsonrpc_routes(handler, "/")
        + create_rest_routes(handler)
    )
    app = Starlette(routes=routes)
    server = uvicorn.Server(
        uvicorn.Config(
            app,
            log_level="error",
            access_log=False,
            lifespan="off",
            limit_concurrency=32,
            timeout_graceful_shutdown=3,
        )
    )
    await grpc_server.start()
    worker = asyncio.create_task(server.serve(sockets=[http_socket]))
    try:
        for _ in range(200):
            if server.started:
                break
            if worker.done():
                await worker
            await asyncio.sleep(0.01)
        if not server.started:
            raise RuntimeError("http startup deadline")
        print(
            json.dumps(
                {
                    "http_url": http_url,
                    "grpc_url": grpc_url,
                    "reference": "official_python_sdk_1.1.3",
                }
            ),
            flush=True,
        )
        await asyncio.to_thread(sys.stdin.readline)
    finally:
        server.should_exit = True
        await grpc_server.stop(3)
        await asyncio.wait_for(worker, 5)
        http_socket.close()


async def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("mode", choices=["client", "serve"])
    parser.add_argument("--url")
    args = parser.parse_args()
    if args.mode == "serve":
        await serve_python_reference()
    else:
        print(json.dumps(await asyncio.wait_for(verify_python_client(args.url), 60)), flush=True)


if __name__ == "__main__":
    asyncio.run(main())

// Command a2a_go_interop exercises official Go SDK clients and a local reference agent.
// It is a bounded synthetic interoperability fixture, not an official ITK suite.
package main

import (
	"bufio"
	"context"
	"crypto/subtle"
	"encoding/json"
	"errors"
	"fmt"
	"io"
	"iter"
	"log/slog"
	"net"
	"net/http"
	"net/url"
	"os"
	"strings"
	"sync/atomic"
	"time"

	"github.com/a2aproject/a2a-go/v2/a2a"
	"github.com/a2aproject/a2a-go/v2/a2aclient"
	"github.com/a2aproject/a2a-go/v2/a2aclient/agentcard"
	a2agrpc "github.com/a2aproject/a2a-go/v2/a2agrpc/v1"
	"github.com/a2aproject/a2a-go/v2/a2asrv"
	"google.golang.org/grpc"
	"google.golang.org/grpc/credentials/insecure"
)

const sdk_version = "v2.5.0"
const sdk_commit = "9d95b95445f4208ba77f48a137a278067937adb7"
const maximum_bootstrap = 8192
const maximum_body = 64 * 1024
const call_deadline = 10 * time.Second

// Only synthetic fixture credentials cross stdin; reports omit credentials and endpoints.
type bootstrap_config struct {
	Http_url     string `json:"http_url"`
	Grpc_url     string `json:"grpc_url"`
	Bearer_token string `json:"bearer_token,omitempty"`
}

type transport_check struct {
	Transport                  string `json:"transport"`
	Task_identity_preserved    bool   `json:"task_identity_preserved"`
	Context_identity_preserved bool   `json:"context_identity_preserved"`
	Get_verified               bool   `json:"get_verified"`
	Cancel_verified            bool   `json:"cancel_verified"`
	Post_cancel_get_verified   bool   `json:"post_cancel_get_verified"`
}

type interop_report struct {
	Direction        string            `json:"direction"`
	Sdk_version      string            `json:"sdk_version"`
	Sdk_commit       string            `json:"sdk_commit"`
	Protocol_version string            `json:"protocol_version"`
	Checks           []transport_check `json:"checks"`
	Official_itk     bool              `json:"official_itk"`
}

func main() {
	slog.SetDefault(slog.New(slog.NewTextHandler(io.Discard, nil)))
	if len(os.Args) != 2 || (os.Args[1] != "client" && os.Args[1] != "server") {
		fail("usage_client_or_server")
	}
	reader := bufio.NewReaderSize(os.Stdin, maximum_bootstrap+1)
	line, err := reader.ReadSlice('\n')
	if err != nil || len(line) > maximum_bootstrap {
		fail("invalid_bootstrap")
	}
	var config bootstrap_config
	if json.Unmarshal(line, &config) != nil || !valid_token(config.Bearer_token) {
		fail("invalid_bootstrap")
	}
	if os.Args[1] == "client" {
		report, err := run_client(config)
		if err != nil {
			fail(err.Error())
		}
		if json.NewEncoder(os.Stdout).Encode(report) != nil {
			fail("report_failed")
		}
		return
	}
	server, err := start_reference_server(config.Bearer_token)
	if err != nil {
		fail("reference_start_failed")
	}
	ready := bootstrap_config{Http_url: server.http_url, Grpc_url: "http://" + server.grpc_address}
	if json.NewEncoder(os.Stdout).Encode(ready) != nil {
		_ = server.close()
		fail("ready_failed")
	}
	command_bytes, read_error := reader.ReadSlice('\n')
	if read_error != nil && !errors.Is(read_error, io.EOF) {
		_ = server.close()
		fail("invalid_shutdown")
	}
	command := string(command_bytes)
	if strings.TrimSpace(command) != "shutdown" && command != "" {
		_ = server.close()
		fail("invalid_shutdown")
	}
	if err := server.close(); err != nil {
		fail("reference_shutdown_failed")
	}
}

func fail(code string) {
	// Error strings below are fixture-owned stable codes, never raw peer payloads.
	_ = json.NewEncoder(os.Stderr).Encode(map[string]string{"error": code})
	os.Exit(1)
}

func valid_token(token string) bool {
	return len(token) >= 16 && len(token) <= 128 && !strings.ContainsAny(token, "\r\n\x00")
}

func validate_http_origin(value string) (string, error) {
	parsed, err := url.Parse(value)
	if err != nil || parsed.Scheme != "http" || parsed.User != nil || parsed.RawQuery != "" || parsed.Fragment != "" || (parsed.Path != "" && parsed.Path != "/") {
		return "", errors.New("invalid_loopback_origin")
	}
	address := net.ParseIP(parsed.Hostname())
	if address == nil || !address.IsLoopback() || parsed.Port() == "" {
		return "", errors.New("invalid_loopback_origin")
	}
	return parsed.Scheme + "://" + parsed.Host, nil
}

// RoundTrip only adds authentication/limits; A2A encoding stays in the official SDK.
type fixture_transport struct {
	base   *http.Transport
	origin string
	token  string
}

func (transport *fixture_transport) RoundTrip(request *http.Request) (*http.Response, error) {
	if request.URL.Scheme+"://"+request.URL.Host != transport.origin {
		return nil, errors.New("fixture_origin_mismatch")
	}
	cloned := request.Clone(request.Context())
	cloned.Header = request.Header.Clone()
	cloned.Header.Set("Authorization", "Bearer "+transport.token)
	response, err := transport.base.RoundTrip(cloned)
	if err != nil {
		return nil, err
	}
	response.Body = &limited_body{Reader: io.LimitReader(response.Body, maximum_body+1), Closer: response.Body}
	return response, nil
}

type limited_body struct {
	io.Reader
	io.Closer
}

func run_client(config bootstrap_config) (interop_report, error) {
	report := interop_report{Direction: "go_sdk_to_rust", Sdk_version: sdk_version, Sdk_commit: sdk_commit, Protocol_version: string(a2a.Version), Official_itk: false}
	origin, err := validate_http_origin(config.Http_url)
	if err != nil || !valid_token(config.Bearer_token) {
		return report, errors.New("invalid_bootstrap")
	}
	grpc_origin, err := validate_http_origin(config.Grpc_url)
	if err != nil {
		return report, err
	}
	grpc_address := strings.TrimPrefix(grpc_origin, "http://")
	base := &http.Transport{Proxy: nil, MaxConnsPerHost: 4, MaxIdleConnsPerHost: 4}
	defer base.CloseIdleConnections()
	http_client := &http.Client{Transport: &fixture_transport{base: base, origin: origin, token: config.Bearer_token}, Timeout: call_deadline,
		CheckRedirect: func(*http.Request, []*http.Request) error { return http.ErrUseLastResponse }}
	ctx, cancel := context.WithTimeout(context.Background(), call_deadline)
	card, err := agentcard.NewResolver(http_client).Resolve(ctx, origin)
	cancel()
	if err != nil {
		return report, errors.New("sdk_card_discovery_failed")
	}
	for _, endpoint := range card.SupportedInterfaces {
		if endpoint == nil {
			return report, errors.New("sdk_card_interface_mismatch")
		}
		expected := origin
		if endpoint.ProtocolBinding == a2a.TransportProtocolGRPC {
			expected = grpc_address
		}
		if strings.TrimRight(endpoint.URL, "/") != expected || endpoint.ProtocolVersion != a2a.Version {
			return report, errors.New("sdk_card_interface_mismatch")
		}
	}
	for _, binding := range []a2a.TransportProtocol{a2a.TransportProtocolHTTPJSON, a2a.TransportProtocolJSONRPC, a2a.TransportProtocolGRPC} {
		check, err := run_binding(card, binding, config.Bearer_token, http_client)
		if err != nil {
			return report, err
		}
		report.Checks = append(report.Checks, check)
	}
	return report, nil
}

func run_binding(card *a2a.AgentCard, binding a2a.TransportProtocol, token string, http_client *http.Client) (transport_check, error) {
	result := transport_check{Transport: string(binding)}
	options := []a2aclient.FactoryOption{a2aclient.WithDefaultsDisabled(), a2aclient.WithConfig(a2aclient.Config{PreferredTransports: []a2a.TransportProtocol{binding}})}
	switch binding {
	case a2a.TransportProtocolHTTPJSON:
		options = append(options, a2aclient.WithRESTTransport(http_client))
	case a2a.TransportProtocolJSONRPC:
		options = append(options, a2aclient.WithJSONRPCTransport(http_client))
	case a2a.TransportProtocolGRPC:
		options = append(options, a2agrpc.WithGRPCTransport(grpc.WithTransportCredentials(insecure.NewCredentials()), grpc.WithDefaultCallOptions(grpc.MaxCallRecvMsgSize(maximum_body), grpc.MaxCallSendMsgSize(maximum_body))))
	}
	ctx, cancel := context.WithTimeout(context.Background(), call_deadline)
	defer cancel()
	ctx = a2aclient.AttachServiceParams(ctx, a2aclient.ServiceParams{"authorization": {"Bearer " + token}})
	client, err := a2aclient.NewFromCard(ctx, card, options...)
	if err != nil {
		return result, fmt.Errorf("%s_client_failed", binding)
	}
	defer client.Destroy()
	message := a2a.NewMessage(a2a.MessageRoleUser, a2a.NewTextPart("tck-input-required-"+a2a.NewMessageID()))
	message.ID = "tck-input-required-" + a2a.NewMessageID()
	message.ContextID = a2a.NewContextID()
	response, err := client.SendMessage(ctx, &a2a.SendMessageRequest{Message: message})
	if err != nil {
		return result, fmt.Errorf("%s_send_failed", binding)
	}
	task, ok := response.(*a2a.Task)
	if !ok || task == nil || task.ID == "" || task.ContextID != message.ContextID || task.Status.State != a2a.TaskStateInputRequired {
		return result, fmt.Errorf("%s_initial_task_mismatch", binding)
	}
	retrieved, err := client.GetTask(ctx, &a2a.GetTaskRequest{ID: task.ID})
	if err != nil || retrieved == nil || retrieved.ID != task.ID || retrieved.ContextID != task.ContextID || retrieved.Status.State != task.Status.State {
		return result, fmt.Errorf("%s_get_failed", binding)
	}
	cancelled, err := client.CancelTask(ctx, &a2a.CancelTaskRequest{ID: task.ID})
	if err != nil || cancelled == nil || cancelled.ID != task.ID || cancelled.ContextID != task.ContextID || cancelled.Status.State != a2a.TaskStateCanceled {
		return result, fmt.Errorf("%s_cancel_failed", binding)
	}
	observed, err := client.GetTask(ctx, &a2a.GetTaskRequest{ID: task.ID})
	if err != nil || observed == nil || observed.ID != task.ID || observed.ContextID != task.ContextID || observed.Status.State != a2a.TaskStateCanceled {
		return result, fmt.Errorf("%s_post_cancel_get_failed", binding)
	}
	result.Task_identity_preserved = true
	result.Context_identity_preserved = true
	result.Get_verified = true
	result.Cancel_verified = true
	result.Post_cancel_get_verified = true
	return result, nil
}

type fixture_auth struct {
	a2asrv.PassthroughCallInterceptor
	token string
	calls atomic.Uint32
}

func (auth *fixture_auth) Before(ctx context.Context, call_ctx *a2asrv.CallContext, request *a2asrv.Request) (context.Context, any, error) {
	values, _ := call_ctx.ServiceParams().Get("authorization")
	if len(values) != 1 || subtle.ConstantTimeCompare([]byte(values[0]), []byte("Bearer "+auth.token)) != 1 {
		return ctx, nil, errors.New("fixture_unauthorized")
	}
	if auth.calls.Add(1) > 64 {
		return ctx, nil, errors.New("fixture_request_limit")
	}
	call_ctx.User = a2asrv.NewAuthenticatedUser("fixture_client", nil)
	return ctx, nil, nil
}

type reference_server struct {
	http_url     string
	grpc_address string
	http_server  *http.Server
	grpc_server  *grpc.Server
	http_done    chan error
	grpc_done    chan error
}

func start_reference_server(token string) (*reference_server, error) {
	if !valid_token(token) {
		return nil, errors.New("invalid_bootstrap")
	}
	http_listener, err := net.Listen("tcp", "127.0.0.1:0")
	if err != nil {
		return nil, err
	}
	grpc_listener, err := net.Listen("tcp", "127.0.0.1:0")
	if err != nil {
		_ = http_listener.Close()
		return nil, err
	}
	server := &reference_server{http_url: "http://" + http_listener.Addr().String(), grpc_address: grpc_listener.Addr().String(), http_done: make(chan error, 1), grpc_done: make(chan error, 1)}
	card := &a2a.AgentCard{Name: "VibeMux Go interoperability fixture", Description: "Synthetic input-required task fixture", Version: "1.0.0",
		SupportedInterfaces: []*a2a.AgentInterface{a2a.NewAgentInterface(server.http_url, a2a.TransportProtocolHTTPJSON), a2a.NewAgentInterface(server.http_url, a2a.TransportProtocolJSONRPC), a2a.NewAgentInterface(server.grpc_address, a2a.TransportProtocolGRPC)},
		DefaultInputModes:   []string{"text/plain", "application/json"}, DefaultOutputModes: []string{"text/plain", "application/json"}, Capabilities: a2a.AgentCapabilities{},
		SecuritySchemes: a2a.NamedSecuritySchemes{"fixture_bearer": a2a.HTTPAuthSecurityScheme{Scheme: "bearer"}}, SecurityRequirements: a2a.SecurityRequirementsOptions{{"fixture_bearer": {}}},
		Skills: []a2a.AgentSkill{{ID: "fixture_task", Name: "Fixture task", Description: "Returns input-required until canceled", Tags: []string{"interop"}}}}
	executor := a2asrv.AgentExecutorFunc(func(ctx context.Context, execution *a2asrv.ExecutorContext) iter.Seq2[a2a.Event, error] {
		return func(yield func(a2a.Event, error) bool) {
			if ctx.Err() != nil {
				yield(nil, ctx.Err())
				return
			}
			task := a2a.NewSubmittedTask(execution, execution.Message)
			task.Status.State = a2a.TaskStateInputRequired
			yield(task, nil)
		}
	})
	handler := a2asrv.NewHandler(executor, a2asrv.WithCallInterceptors(&fixture_auth{token: token}))
	rest := a2asrv.NewRESTHandler(handler)
	rpc := a2asrv.NewJSONRPCHandler(handler)
	discovery := a2asrv.NewStaticAgentCardHandler(card)
	router := http.HandlerFunc(func(writer http.ResponseWriter, request *http.Request) {
		request.Body = http.MaxBytesReader(writer, request.Body, maximum_body)
		if request.URL.Path == a2asrv.WellKnownAgentCardPath {
			discovery.ServeHTTP(writer, request)
			return
		}
		if request.URL.Path == "/" {
			rpc.ServeHTTP(writer, request)
			return
		}
		rest.ServeHTTP(writer, request)
	})
	server.http_server = &http.Server{Handler: router, ReadHeaderTimeout: call_deadline, ReadTimeout: call_deadline, WriteTimeout: call_deadline, IdleTimeout: call_deadline, MaxHeaderBytes: 16 * 1024}
	server.grpc_server = grpc.NewServer(grpc.MaxRecvMsgSize(maximum_body), grpc.MaxSendMsgSize(maximum_body), grpc.MaxConcurrentStreams(8))
	a2agrpc.NewHandler(handler).RegisterWith(server.grpc_server)
	go func() { server.http_done <- server.http_server.Serve(http_listener) }()
	go func() { server.grpc_done <- server.grpc_server.Serve(grpc_listener) }()
	return server, nil
}

func (server *reference_server) close() error {
	ctx, cancel := context.WithTimeout(context.Background(), 5*time.Second)
	defer cancel()
	http_error := server.http_server.Shutdown(ctx)
	grpc_stopped := make(chan struct{})
	go func() { server.grpc_server.GracefulStop(); close(grpc_stopped) }()
	select {
	case <-grpc_stopped:
	case <-ctx.Done():
		server.grpc_server.Stop()
		<-grpc_stopped
	}
	http_result := <-server.http_done
	grpc_result := <-server.grpc_done
	if http_error != nil || (http_result != nil && !errors.Is(http_result, http.ErrServerClosed)) || grpc_result != nil {
		return errors.New("reference_shutdown_failed")
	}
	return nil
}

package main

import (
	"bufio"
	"bytes"
	"context"
	"encoding/json"
	"fmt"
	"io"
	"net"
	"os"
	"os/exec"
	"path/filepath"
	"testing"
	"time"
)

// Public synthetic credential shared only with the dedicated Rust test fixture.
const fixture_bearer = "vibemux_tck_fixture_token_01234567890123456789"

type owned_process struct {
	command *exec.Cmd
	input   io.WriteCloser
	output  *bufio.Reader
	result  chan error
	stopped bool
}

func fixture_environment() []string {
	values := []string{}
	for _, name := range []string{"SystemRoot", "WINDIR", "PATH", "TEMP", "TMP"} {
		if value, ok := os.LookupEnv(name); ok {
			values = append(values, name+"="+value)
		}
	}
	return values
}

func required_binary(t *testing.T, variable string) string {
	t.Helper()
	path := os.Getenv(variable)
	if path == "" {
		t.Skipf("set %s to the built synthetic Rust fixture", variable)
	}
	info, err := os.Stat(path)
	if err != nil || !filepath.IsAbs(path) || info.IsDir() {
		t.Fatalf("invalid %s fixture executable", variable)
	}
	return path
}

func start_rust_sut(t *testing.T) (*owned_process, bootstrap_config) {
	t.Helper()
	command := exec.Command(required_binary(t, "VIBEMUX_RUST_SUT"))
	configure_child(command)
	command.Env = fixture_environment()
	command.Stderr = io.Discard
	input, err := command.StdinPipe()
	if err != nil {
		t.Fatal("fixture stdin failed")
	}
	output, err := command.StdoutPipe()
	if err != nil {
		t.Fatal("fixture stdout failed")
	}
	if err := command.Start(); err != nil {
		t.Fatal("fixture startup failed")
	}
	process := &owned_process{command: command, input: input, output: bufio.NewReaderSize(output, maximum_bootstrap+1), result: make(chan error, 1)}
	go func() { process.result <- command.Wait() }()
	t.Cleanup(func() { process.stop(t) })
	type ready_result struct {
		line []byte
		err  error
	}
	ready := make(chan ready_result, 1)
	go func() { line, err := process.output.ReadSlice('\n'); ready <- ready_result{line, err} }()
	var config bootstrap_config
	select {
	case message := <-ready:
		if message.err != nil || len(message.line) > maximum_bootstrap || json.Unmarshal(message.line, &config) != nil {
			t.Fatal("fixture readiness failed")
		}
	case <-time.After(10 * time.Second):
		_ = command.Process.Kill()
		t.Fatal("fixture readiness deadline")
	}
	config.Bearer_token = fixture_bearer
	return process, config
}

func (process *owned_process) stop(t *testing.T) {
	t.Helper()
	if process.stopped {
		return
	}
	process.stopped = true
	_, _ = io.WriteString(process.input, "shutdown\n")
	_ = process.input.Close()
	select {
	case err := <-process.result:
		if err != nil {
			t.Error("owned Rust fixture did not exit successfully")
		}
	case <-time.After(8 * time.Second):
		_ = process.command.Process.Kill()
		select {
		case <-process.result:
		case <-time.After(4 * time.Second):
			t.Error("owned Rust fixture reap deadline")
		}
		t.Error("owned Rust fixture required forced termination")
	}
}

func assert_listener_closed(t *testing.T, endpoint string) {
	t.Helper()
	origin, err := validate_http_origin(endpoint)
	if err != nil {
		t.Fatal("invalid listener endpoint")
	}
	address := origin[len("http://"):]
	connection, err := net.DialTimeout("tcp", address, 250*time.Millisecond)
	if err == nil {
		_ = connection.Close()
		t.Error("fixture listener remains reachable after shutdown")
	}
}

func Test_go_sdk_to_rust_sut(t *testing.T) {
	process, config := start_rust_sut(t)
	report, err := run_client(config)
	if err != nil {
		t.Fatal(err)
	}
	if len(report.Checks) != 3 || report.Official_itk {
		t.Fatal("incomplete or mislabeled custom interoperability evidence")
	}
	encoded, _ := json.Marshal(report)
	t.Log(string(encoded))
	process.stop(t)
	assert_listener_closed(t, config.Http_url)
	assert_listener_closed(t, config.Grpc_url)
}

func Test_rust_client_to_go_reference(t *testing.T) {
	executable := required_binary(t, "VIBEMUX_RUST_INTEROP_CLIENT")
	server, err := start_reference_server(fixture_bearer)
	if err != nil {
		t.Fatal("Go reference startup failed")
	}
	closed := false
	t.Cleanup(func() {
		if !closed {
			if err := server.close(); err != nil {
				t.Error(err)
			}
		}
	})
	config := bootstrap_config{Http_url: server.http_url, Grpc_url: "http://" + server.grpc_address}
	input, _ := json.Marshal(config)
	input = append(input, '\n')
	ctx, cancel := context.WithTimeout(context.Background(), 45*time.Second)
	defer cancel()
	command := exec.CommandContext(ctx, executable)
	configure_child(command)
	command.Env = fixture_environment()
	command.Stdin = bytes.NewReader(input)
	var stdout capped_buffer
	var stderr capped_buffer
	command.Stdout = &stdout
	command.Stderr = &stderr
	if err := command.Run(); err != nil {
		t.Fatalf("Rust client fixture failed: %s", stderr.String())
	}
	var report struct {
		Direction string `json:"direction"`
		Checks    []struct {
			Binding           string `json:"binding"`
			Send_get_identity bool   `json:"send_get_identity"`
			Cancel_terminal   bool   `json:"cancel_terminal"`
		} `json:"checks"`
	}
	if json.Unmarshal(stdout.Bytes(), &report) != nil || len(report.Checks) != 3 {
		t.Fatal("invalid Rust fixture report")
	}
	for _, check := range report.Checks {
		if !check.Send_get_identity || !check.Cancel_terminal {
			t.Fatalf("missing Rust check for %s", check.Binding)
		}
	}
	t.Logf("%s", stdout.String())
	if err := server.close(); err != nil {
		t.Fatal(err)
	}
	closed = true
	assert_listener_closed(t, config.Http_url)
	assert_listener_closed(t, config.Grpc_url)
}

type capped_buffer struct{ buffer bytes.Buffer }

func (buffer *capped_buffer) Bytes() []byte  { return buffer.buffer.Bytes() }
func (buffer *capped_buffer) String() string { return buffer.buffer.String() }

func (buffer *capped_buffer) Write(input []byte) (int, error) {
	remaining := maximum_body - buffer.buffer.Len()
	keep := len(input)
	if keep > remaining {
		keep = remaining
	}
	if keep > 0 {
		_, _ = buffer.buffer.Write(input[:keep])
	}
	return len(input), nil
}

func Test_loopback_only_bootstrap_rejects_external_destinations(t *testing.T) {
	for _, target := range []string{"https://127.0.0.1:8080", "http://example.invalid:8080", "http://127.0.0.1:8080/path", "http://user@127.0.0.1:8080", "http://127.0.0.1:8080?token=x"} {
		if _, err := validate_http_origin(target); err == nil {
			t.Error(fmt.Sprintf("accepted invalid fixture origin %q", target))
		}
	}
}

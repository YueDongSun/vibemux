//go:build windows

package main

import (
	"os/exec"
	"syscall"
)

func configure_child(command *exec.Cmd) {
	command.SysProcAttr = &syscall.SysProcAttr{HideWindow: true}
}

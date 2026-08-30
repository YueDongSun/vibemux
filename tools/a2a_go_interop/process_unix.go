//go:build !windows

package main

import "os/exec"

func configure_child(command *exec.Cmd) {}

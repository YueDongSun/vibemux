# Mock workflow lab

In a temporary repo with an initial commit, run `vibemux init --terminal-backend mock`, create a task, spawn two runs, send `PING` and `WRITE file.txt` to each, then inspect the diff and trace. This experiment does not access the network, does not call any model, and does not touch the user's terminal.

# Mock workflow lab

在 initial commit 的临时 repo 中运行 `vibemux init --terminal-backend mock`，创建 task，spawn 两个 run，分别发送 `PING`、`WRITE file.txt`，检查 diff 和 trace。该实验不联网、不调用模型、不触碰用户 terminal。


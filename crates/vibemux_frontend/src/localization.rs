#![forbid(unsafe_code)]
//! Offline GUI text catalog. Raw task content and technical identifiers are data.

use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
pub enum Language {
    #[serde(rename = "zh_cn")]
    Chinese,
    #[default]
    #[serde(rename = "en", other)]
    English,
}

impl Language {
    pub const ALL: [Self; 2] = [Self::English, Self::Chinese];

    pub const fn code(self) -> &'static str {
        match self {
            Self::English => "EN",
            Self::Chinese => "CN",
        }
    }

    pub fn text(self, english: &str) -> &str {
        if self == Self::English {
            return english;
        }
        MESSAGES
            .iter()
            .find_map(|(key, translated)| (*key == english).then_some(*translated))
            .unwrap_or(english)
    }

    /// Expand only placeholders in the catalog template, never inserted values.
    pub fn format(self, english: &str, arguments: &[(&str, &str)]) -> String {
        let mut remaining = self.text(english);
        let mut output = String::new();
        while let Some(start) = remaining.find('{') {
            let Some(end) = remaining[start..].find('}').map(|end| start + end) else {
                break;
            };
            output.push_str(&remaining[..start]);
            let name = &remaining[start + 1..end];
            let value = arguments
                .iter()
                .find_map(|(key, value)| (*key == name).then_some(*value));
            output.push_str(value.unwrap_or(&remaining[start..=end]));
            remaining = &remaining[end + 1..];
        }
        output.push_str(remaining);
        output
    }
}

pub const MESSAGES: &[(&str, &str)] = &[
    ("Claude terminal", "Claude 终端"),
    ("Start Claude", "启动 Claude"),
    ("Stop session", "停止会话"),
    ("Starting terminal…", "正在启动终端…"),
    ("Stopping…", "正在停止…"),
    ("Not started", "尚未启动"),
    ("Session exited", "会话已退出"),
    ("OFFLINE PTY FIXTURE", "离线 PTY 测试"),
    ("Terminal error", "终端错误"),
    ("Isolated branch", "独立分支"),
    (
        "Claude's native TUI runs here. Click the terminal to type; Claude handles its own permission prompts.",
        "Claude 原生界面在此运行。点击终端输入；权限提示由 Claude 自行处理。",
    ),
    (
        "Start Claude to open an interactive native terminal.",
        "启动 Claude 以打开交互式原生终端。",
    ),
    ("Language", "语言"),
    ("English", "英语"),
    ("Chinese (Simplified)", "简体中文"),
    ("Interface language", "界面语言"),
    (
        "Applies immediately. Your task content stays in its original language.",
        "立即生效。任务内容保留原始语言。",
    ),
    ("Settings", "设置"),
    ("Appearance", "外观"),
    ("Choose a palette for your workspace.", "选择工作区配色。"),
    ("Startup window size", "启动窗口大小"),
    ("Width", "宽度"),
    ("Height", "高度"),
    ("Saved for the next launch.", "已保存，下次启动时生效。"),
    ("New task", "新建任务"),
    ("New task draft", "新建任务草稿"),
    ("Conversation", "对话"),
    ("Agents", "智能体"),
    ("Recents", "最近任务"),
    ("Search anything   Ctrl+K", "搜索   Ctrl+K"),
    ("Search anything (Ctrl+K)", "搜索（Ctrl+K）"),
    ("Collapse sidebar", "收起侧边栏"),
    ("Expand sidebar", "展开侧边栏"),
    ("Toggle sidebar", "切换侧边栏"),
    ("Local workspace", "本地工作区"),
    ("Workspace menu", "工作区菜单"),
    ("Tasks appear here when assigned.", "分配的任务将在此显示。"),
    ("Untitled task", "未命名任务"),
    ("Unassigned", "未分配"),
    ("Main conversation", "主对话"),
    ("Coordinator", "协调者"),
    ("Refresh", "刷新"),
    ("Connected", "已连接"),
    ("Connecting", "正在连接"),
    ("Disconnected", "已断开"),
    ("Unavailable", "不可用"),
    ("DEMO", "演示"),
    ("DEMO DATA", "演示数据"),
    ("Workspace updates", "工作区动态"),
    (
        "Task activity · reported by the daemon",
        "任务动态 · 由守护进程报告",
    ),
    (
        "{count} tasks in this workspace",
        "此工作区有 {count} 个任务",
    ),
    (
        "Open a task to follow its execution. The conversation stays here.",
        "打开任务以查看执行情况，对话将保留在此处。",
    ),
    ("Load more tasks", "加载更多任务"),
    ("Open task details", "打开任务详情"),
    ("Details", "详情"),
    ("Open window", "独立窗口"),
    ("Native terminal", "本机终端"),
    ("Good morning", "早上好"),
    ("Good afternoon", "下午好"),
    ("Good evening", "晚上好"),
    ("Hello", "你好"),
    ("How can I help today?", "今天需要什么帮助？"),
    ("Draft only", "仅为草稿"),
    ("Draft for {name}", "发给 {name} 的草稿"),
    (
        "{draft} · coordinator chat is not connected",
        "{draft} · 协调者对话尚未连接",
    ),
    (
        "Not sent: coordinator chat is not connected",
        "未发送：协调者对话尚未连接",
    ),
    (
        "Sending is unavailable because no continuous chat service is connected.",
        "尚未连接连续对话服务，暂时无法发送。",
    ),
    ("Not detected on this machine", "未在本机检测到"),
    ("Send", "发送"),
    ("Discard current draft?", "放弃当前草稿？"),
    (
        "The draft has not been sent and will be removed.",
        "草稿尚未发送，放弃后将被清除。",
    ),
    ("Discard", "放弃"),
    ("Keep editing", "继续编辑"),
    ("Jump to anything", "快速导航"),
    (
        "Search tasks, IDs, agents or actions…",
        "搜索任务、标识、智能体或操作…",
    ),
    (
        "No matches in the loaded workspace",
        "已加载的工作区中没有匹配项",
    ),
    (
        "Try a task title, ID, harness name or Settings.",
        "试试任务标题、标识、执行器名称或设置。",
    ),
    (
        "↑ ↓ to navigate    Enter to open    Esc to close",
        "↑ ↓ 选择    Enter 打开    Esc 关闭",
    ),
    (
        "Searching loaded tasks only. Load more in Conversation for older tasks.",
        "仅搜索已加载的任务。可在对话中加载更早的任务。",
    ),
    ("Task · {executor} · {id}", "任务 · {executor} · {id}"),
    ("Workspace action", "工作区操作"),
    ("Agent · inspect availability", "智能体 · 查看可用性"),
    ("Light theme", "浅色主题"),
    ("Dark theme", "深色主题"),
    ("Keyboard shortcuts", "键盘快捷键"),
    ("Keyboard shortcuts    Ctrl+/", "键盘快捷键    Ctrl+/"),
    ("Find a task, agent or action", "查找任务、智能体或操作"),
    ("Inspect {agent}", "查看 {agent}"),
    (
        "Esc closes the current overlay. Agent shortcuts pause while typing.",
        "Esc 关闭当前面板。输入文字时暂停智能体快捷键。",
    ),
    ("Done", "完成"),
    ("Copy", "复制"),
    ("Copied", "已复制"),
    ("Copy {label}", "复制{label}"),
    ("task title", "任务标题"),
    ("Close task details", "关闭任务详情"),
    ("Overview", "概览"),
    ("Activity", "动态"),
    ("Artifacts", "产物"),
    ("Terminal", "终端"),
    ("No Run is recorded for this task.", "此任务尚无执行记录。"),
    ("Run · {harness} · {id}", "执行 · {harness} · {id}"),
    ("Select Run", "选择执行记录"),
    ("LATEST UPDATE", "最新动态"),
    ("RUN DETAILS", "执行详情"),
    ("RECENT ACTIVITY", "近期动态"),
    ("No event summary is available yet.", "暂无事件摘要。"),
    ("Harness", "执行器"),
    ("Role", "角色"),
    ("Run state", "执行状态"),
    ("Branch", "分支"),
    ("Worktree", "工作树"),
    ("Task", "任务"),
    ("Run", "执行"),
    ("Unknown", "未知"),
    ("Not recorded", "未记录"),
    ("Not inspected", "未检查"),
    (
        "Run details will appear when a Run is recorded.",
        "创建执行记录后将在此显示详情。",
    ),
    ("Identity & references", "标识与引用"),
    (
        "Showing the 12 most recent Runs.",
        "显示最近的 12 条执行记录。",
    ),
    ("No activity events are available.", "暂无动态事件。"),
    ("No event summary was supplied.", "未提供事件摘要。"),
    (
        "Showing the 24 most recent events.",
        "显示最近的 24 条事件。",
    ),
    ("ARTIFACTS", "产物"),
    (
        "Select a Run to inspect its artifacts.",
        "选择执行记录以查看产物。",
    ),
    (
        "No artifacts are recorded for this Run.",
        "此次执行尚无产物记录。",
    ),
    ("Unnamed artifact", "未命名产物"),
    ("Unknown type", "未知类型"),
    ("Unknown state", "未知状态"),
    (
        "Task artifact list is limited to 24 references.",
        "任务产物列表最多显示 24 条引用。",
    ),
    ("NATIVE TERMINAL BINDING", "本机终端关联"),
    (
        "Select a Run before inspecting a terminal.",
        "检查终端前请选择执行记录。",
    ),
    ("Status", "状态"),
    ("Binding", "关联"),
    ("Checked", "检查时间"),
    ("Focus terminal", "定位终端"),
    ("Unlink terminal", "取消关联"),
    ("Inspect terminal", "检查终端"),
    ("AVAILABLE TERMINALS", "可用终端"),
    (
        "No terminal candidates are reported.",
        "未发现可关联的终端。",
    ),
    (
        "Terminal contents stay in the native terminal surface; this view shows binding metadata only.",
        "终端内容保留在本机终端中；此处仅显示关联信息。",
    ),
    ("Terminal candidate", "候选终端"),
    ("Plugin", "插件"),
    ("Pane", "窗格"),
    ("Instance", "实例"),
    ("Link terminal", "关联终端"),
    (
        "Link this exact reported terminal candidate to the selected Run.",
        "将此候选终端关联到所选执行记录。",
    ),
    ("Task observation", "任务观察"),
    ("Task observation unavailable", "任务观察不可用"),
    (
        "Task window state is temporarily unavailable.",
        "任务窗口状态暂时不可用。",
    ),
    (
        "This task is absent from the current page or connection. Its execution has not been changed.",
        "当前页面或连接中没有此任务。任务执行状态未改变。",
    ),
    ("Refresh task", "刷新任务"),
    (
        "Read-only launcher and route diagnostics from the local probe.",
        "来自本地探测的启动器与路由只读诊断。",
    ),
    (
        "No agent probe results are available.",
        "暂无智能体探测结果。",
    ),
    (
        "Launcher: {state}  ·  Route: {route}",
        "启动器：{state}  ·  路由：{route}",
    ),
    ("←  All agents", "←  全部智能体"),
    ("Launcher", "启动器"),
    ("Authentication", "身份验证"),
    ("Inference probe", "推理探测"),
    ("Version", "版本"),
    ("Route", "路由"),
    ("Result code", "结果代码"),
    (
        "Probe results do not start an agent or open a terminal.",
        "查看探测结果不会启动智能体或打开终端。",
    ),
    ("Verified", "已验证"),
    ("Failed", "失败"),
    ("Not run", "未运行"),
    ("Diagnostics", "诊断"),
    ("SUPERVISOR", "监督器"),
    ("Connection", "连接"),
    ("Project", "项目"),
    ("Tasks in page", "当前页任务数"),
    ("More tasks", "更多任务"),
    ("available", "可用"),
    ("none reported", "暂无报告"),
    ("LOCAL PROBE", "本地探测"),
    ("Platform", "平台"),
    ("Gateway", "网关"),
    ("A2A self-test", "A2A 自检"),
    ("Observed at", "观测时间"),
    ("Aggregate health", "总体健康状态"),
    ("ALLOWLISTED ENVIRONMENT", "允许的环境变量"),
    ("set", "已设置"),
    ("unset", "未设置"),
    (
        "No allowlisted environment values were reported.",
        "未报告允许的环境变量。",
    ),
    (
        "Diagnostics show probe summaries and allowlisted variable presence; values and prompt content are not displayed.",
        "诊断仅显示探测摘要及允许变量是否存在，不显示变量值或提示词内容。",
    ),
    ("Completed", "已完成"),
    ("In progress", "进行中"),
    ("Needs input", "需要输入"),
    ("Open", "待处理"),
    ("Running", "运行中"),
    ("Blocked", "受阻"),
    ("Cancelled", "已取消"),
    ("Queued", "排队中"),
    ("Succeeded", "已成功"),
    ("Pending", "等待中"),
    (
        "Action queue is full. The last action was not sent; please try again.",
        "操作队列已满。上次操作未发送，请重试。",
    ),
    (
        "Preview: task status connected; continuous coordinator chat is unavailable.",
        "演示：任务状态已连接；连续协调者对话尚不可用。",
    ),
    (
        "The supervisor runtime is not connected.",
        "监督器运行时尚未连接。",
    ),
];

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeSet;

    fn placeholders(text: &str) -> BTreeSet<&str> {
        text.split('{')
            .skip(1)
            .filter_map(|part| part.split_once('}').map(|(name, _)| name))
            .collect()
    }

    #[test]
    fn catalog_is_complete_unique_and_preserves_parameters() {
        let mut keys = BTreeSet::new();
        for (english, chinese) in MESSAGES {
            assert!(keys.insert(english), "duplicate: {english}");
            assert!(!chinese.trim().is_empty(), "{english}");
            assert_eq!(placeholders(english), placeholders(chinese), "{english}");
            assert_eq!(Language::English.text(english), *english);
            assert_eq!(Language::Chinese.text(english), *chinese);
        }
    }

    #[test]
    fn substitutions_are_literal_and_unknown_text_is_preserved() {
        let title = "{id} 東京 <script>";
        assert_eq!(
            Language::Chinese.format("Copy {label}", &[("label", title)]),
            format!("复制{title}")
        );
        assert_eq!(
            Language::Chinese.text("project/custom_task"),
            "project/custom_task"
        );
        assert_eq!(Language::default(), Language::English);
    }
}

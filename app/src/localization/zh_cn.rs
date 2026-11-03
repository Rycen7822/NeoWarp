//! Statically compiled UI messages, keyed by their English source text.

pub(super) fn lookup(english: &str) -> Option<&'static str> {
    Some(match english {
        "Appearance" => "外观",
        "Features" => "功能",
        "Keyboard shortcuts" => "键盘快捷键",
        "Privacy" => "隐私",
        "About" => "关于",
        "Editor" => "编辑器",
        "Code" => "代码",
        "Language" => "语言",
        "Interface language" => "界面语言",
        "Files" => "文件",
        "Projects" => "项目",
        "Settings" => "设置",
        _ => return None,
    })
}

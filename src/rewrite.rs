/*
 * rewrite.rs
 * Project: sts-x
 * Description: P0-1 0 命中自动重试链 — 本地中英代码词典 + 符号猜测 + file 兜底编排。
 *
 * Why: 中文 NL 查询经 bigram 分词后的 term 在英文代码库（95%+ 英文标识符）里
 * 不存在 → BM25 必然 0 命中，旧行为只甩出 `hint` 引导文本，AI 需手动换词再调，
 * 中文弱时死循环。本模块在 0 命中时按固定顺序自动重试（单次调用即出最佳结果）：
 *   1. EnglishSynonym — 中英代码词典映射（缓存→cache、索引→index…）；
 *   2. SymbolGuess   — 去停用词后抽 ASCII 标识符再查（如 "hint"）；
 *   3. FileFallback  — `sts-x file` 内容兜底（文件名/内容子串匹配）。
 *
 * 纯本地、零新依赖、零网络。只做查询改写与编排，不碰 BM25 / 分词 / 路由。
 */

/// 重试阶段类型。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RetryKind {
    /// 中英词典映射的英文词直接 BM25（最可靠，优先）
    EnglishSynonym,
    /// 去停用词后抽 ASCII 标识符 BM25
    SymbolGuess,
    /// 文件名/内容子串兜底（rg / ignore walker）
    FileFallback,
}

impl RetryKind {
    pub fn label(&self) -> &'static str {
        match self {
            Self::EnglishSynonym => "english-synonym",
            Self::SymbolGuess => "symbol-guess",
            Self::FileFallback => "file-fallback",
        }
    }
}

/// True for CJK ideographs / kana / hangul. Mirrors `indexer::is_cjk`.
fn is_cjk(c: char) -> bool {
    matches!(c,
        '\u{3400}'..='\u{4dbf}' | // CJK Ext A
        '\u{4e00}'..='\u{9fff}' | // CJK Unified
        '\u{3040}'..='\u{30ff}' | // Hiragana / Katakana
        '\u{ac00}'..='\u{d7af}'   // Hangul
    )
}

fn has_cjk(query: &str) -> bool {
    query.chars().any(is_cjk)
}

/// 中英代码词典（本地，纯静态）。覆盖代码领域高频中文词 → 英文符号/标识符。
/// 匹配方式：`query.contains(zh)`，因此顺序无关；英文词去重保序。
static CODE_DICT: &[(&str, &[&str])] = &[
    ("缓存", &["cache"]),
    ("索引", &["index", "indexer"]),
    ("窗口", &["window"]),
    ("路由", &["router", "route"]),
    ("提示", &["hint"]),
    ("攻略", &["hint", "guide"]),
    ("自救", &["hint", "fallback"]),
    ("搜索", &["search"]),
    ("查找", &["find", "locate", "search"]),
    ("文件", &["file", "filesearch"]),
    ("目录", &["directory", "dir"]),
    ("路径", &["path"]),
    ("模块", &["module", "mod"]),
    ("函数", &["function", "fn"]),
    ("方法", &["method"]),
    ("类", &["class"]),
    ("结构体", &["struct"]),
    ("结构", &["struct", "structure"]),
    ("接口", &["interface", "trait"]),
    ("枚举", &["enum"]),
    ("错误", &["error"]),
    ("异常", &["exception", "error"]),
    ("配置", &["config"]),
    ("构建", &["build"]),
    ("编译", &["compile"]),
    ("测试", &["test"]),
    ("解析", &["parse", "parser"]),
    ("模型", &["model"]),
    ("嵌入", &["embed", "embedding"]),
    ("向量", &["vector"]),
    ("分词", &["tokenizer", "token", "tokenize"]),
    ("令牌", &["token"]),
    ("重试", &["retry"]),
    ("命中", &["hit", "match"]),
    ("结果", &["result"]),
    ("修复", &["fix", "repair"]),
    ("编码", &["encode"]),
    ("解码", &["decode"]),
    ("项目", &["project"]),
    ("分块", &["chunk", "chunker"]),
    ("切块", &["chunk", "chunker"]),
    ("语法", &["syntax"]),
    ("语言", &["language"]),
    ("服务", &["server", "service"]),
    ("客户端", &["client"]),
    ("请求", &["request"]),
    ("响应", &["response"]),
    ("查询", &["query"]),
    ("排序", &["sort", "rank"]),
    ("过滤", &["filter"]),
    ("匹配", &["match"]),
    ("相似", &["similar", "similarity"]),
    ("分数", &["score"]),
    ("行号", &["line"]),
    ("上下文", &["context"]),
    ("预算", &["budget"]),
    ("中文", &["cjk", "chinese"]),
    ("英文", &["ascii", "english"]),
    ("符号", &["symbol", "ident"]),
    ("标识符", &["identifier", "symbol"]),
    ("名称", &["name"]),
    ("签名", &["signature"]),
    ("注释", &["comment", "doc_comment"]),
    ("文档", &["doc", "documentation"]),
    ("代码", &["code"]),
    ("源码", &["source", "code"]),
    ("命令", &["command", "cmd"]),
    ("参数", &["arg", "argument", "param"]),
    ("选项", &["option", "flag"]),
    ("默认", &["default"]),
    ("日志", &["log", "tracing"]),
    ("跟踪", &["trace", "tracing"]),
    ("调试", &["debug"]),
    ("发布", &["release", "publish"]),
    ("版本", &["version"]),
    ("升级", &["upgrade", "bump"]),
    ("打包", &["package", "pack"]),
    ("二进制", &["binary"]),
    ("数据库", &["database", "db"]),
    ("读取", &["read"]),
    ("写入", &["write"]),
    ("输出", &["output"]),
    ("输入", &["input"]),
    ("格式", &["format"]),
    ("类型", &["type"]),
    ("合并", &["merge"]),
    ("去噪", &["denoise", "postprocess"]),
    ("后处理", &["postprocess"]),
    ("关键词", &["keyword", "term"]),
    ("停用词", &["stopword"]),
    ("设置", &["setting", "config"]),
    ("执行", &["run", "execute"]),
    ("调用", &["call", "invoke"]),
    ("加载", &["load"]),
    ("保存", &["save"]),
    ("打开", &["open"]),
    ("关闭", &["close"]),
    ("检查", &["check", "verify"]),
    ("验证", &["verify", "validate"]),
    ("转换", &["convert", "transform"]),
    ("删除", &["remove", "delete"]),
    ("新增", &["add", "insert"]),
    ("更新", &["update"]),
    ("实现", &["impl", "implement"]),
    ("递归", &["recursive"]),
    ("长度", &["len", "length"]),
    ("大小", &["size"]),
    ("数量", &["count", "num"]),
    ("阈值", &["threshold"]),
    ("权重", &["weight"]),
    ("归一化", &["normalize"]),
    ("相似度", &["similarity"]),
    ("召回", &["recall"]),
    ("精确", &["precision", "exact"]),
    ("布尔", &["bool"]),
    ("字符串", &["string"]),
    ("数组", &["array", "vec"]),
    ("列表", &["list"]),
    ("字典", &["map", "dict"]),
    ("集合", &["set"]),
    ("映射", &["map"]),
    ("队列", &["queue"]),
    ("栈", &["stack"]),
    ("树", &["tree"]),
    ("节点", &["node"]),
    ("图", &["graph"]),
    ("链接", &["link"]),
    ("连接", &["connect"]),
    ("网络", &["network"]),
    ("协议", &["protocol"]),
    ("端口", &["port"]),
    ("地址", &["address"]),
    ("线程", &["thread"]),
    ("进程", &["process"]),
    ("任务", &["task"]),
    ("同步", &["sync"]),
    ("异步", &["async"]),
    ("锁", &["lock"]),
    ("互斥", &["mutex"]),
    ("事件", &["event"]),
    ("回调", &["callback"]),
    ("钩子", &["hook"]),
    ("插件", &["plugin"]),
    ("扩展", &["extension", "plugin"]),
    ("依赖", &["dependency", "dep"]),
    ("分支", &["branch"]),
    ("提交", &["commit"]),
    ("仓库", &["repo", "repository"]),
    ("工作区", &["workspace"]),
    ("特性", &["feature"]),
    ("功能", &["feature", "function"]),
    ("元数据", &["metadata"]),
    ("模式", &["pattern", "mode"]),
    ("策略", &["strategy", "policy"]),
    ("算法", &["algorithm"]),
    ("优化", &["optimize"]),
    ("性能", &["performance"]),
    ("速度", &["speed"]),
    ("延迟", &["latency"]),
    ("内存", &["memory"]),
    ("文件系统", &["filesystem", "fs"]),
    ("权限", &["permission"]),
    ("安全", &["security", "safe"]),
    ("密钥", &["key"]),
    ("会话", &["session"]),
    ("用户", &["user"]),
    ("管理", &["admin", "manage"]),
    ("登录", &["login"]),
    ("注册", &["register"]),
    ("模板", &["template"]),
    ("组件", &["component"]),
    ("控件", &["widget"]),
    ("样式", &["style", "css"]),
    ("字体", &["font"]),
    ("颜色", &["color"]),
    ("图片", &["image"]),
    ("内容", &["content"]),
    ("标题", &["title"]),
    ("头部", &["header"]),
    ("底部", &["footer"]),
    ("侧边栏", &["sidebar"]),
    ("导航", &["nav", "navigation"]),
    ("布局", &["layout"]),
    ("状态", &["state"]),
    ("数据", &["data"]),
    ("字段", &["field"]),
    ("属性", &["attribute", "property"]),
    ("记录", &["record"]),
    ("列", &["column"]),
    ("单元格", &["cell"]),
    ("值", &["value"]),
    ("键", &["key"]),
    ("条目", &["entry", "item"]),
    ("范围", &["range"]),
    ("限制", &["limit"]),
    ("最大", &["max"]),
    ("最小", &["min"]),
    ("总数", &["total"]),
    ("汇总", &["summary"]),
    ("统计", &["stat", "statistics"]),
    ("增量", &["increment", "delta"]),
    ("常量", &["constant", "const"]),
    ("变量", &["variable", "var"]),
    ("全局", &["global"]),
    ("局部", &["local"]),
    ("静态", &["static"]),
    ("动态", &["dynamic"]),
    ("实例", &["instance"]),
    ("对象", &["object"]),
    ("代理", &["proxy"]),
    ("迭代器", &["iterator", "iter"]),
    ("流", &["stream"]),
    ("管道", &["pipe"]),
    ("通道", &["channel"]),
    ("缓冲", &["buffer"]),
    ("池", &["pool"]),
    ("临时", &["temp", "temporary"]),
    ("持久", &["persist", "persistent"]),
    ("序列化", &["serialize"]),
    ("反序列化", &["deserialize"]),
    ("压缩", &["compress"]),
    ("解压", &["decompress"]),
    ("哈希", &["hash"]),
    ("摘要", &["digest"]),
    ("导出", &["export"]),
    ("导入", &["import"]),
    ("下载", &["download"]),
    ("上传", &["upload"]),
    ("复制", &["copy"]),
    ("移动", &["move"]),
    ("重命名", &["rename"]),
    ("文件名", &["filename"]),
    ("扩展名", &["extension", "ext"]),
    ("转义", &["escape"]),
    ("分隔符", &["separator", "delimiter"]),
    ("拼接", &["join", "concat"]),
    ("拆分", &["split"]),
    ("替换", &["replace"]),
    ("修剪", &["trim"]),
    ("去重", &["dedup", "unique"]),
    ("切片", &["slice"]),
    ("迭代", &["iterate", "iter"]),
    ("遍历", &["traverse", "iter"]),
    ("聚合", &["aggregate"]),
    ("分组", &["group"]),
    ("计数", &["count"]),
    ("求和", &["sum"]),
    ("条件", &["condition"]),
    ("循环", &["loop"]),
    ("返回", &["return"]),
    ("断言", &["assert"]),
    ("打印", &["print"]),
    ("追加", &["append"]),
    ("截断", &["truncate"]),
    ("刷新", &["flush", "refresh"]),
    ("监听", &["listen"]),
    ("绑定", &["bind"]),
    ("接受", &["accept"]),
    ("发送", &["send"]),
    ("接收", &["receive"]),
    ("订阅", &["subscribe"]),
    ("通知", &["notify"]),
    ("等待", &["wait"]),
    ("超时", &["timeout"]),
    ("取消", &["cancel"]),
    ("中断", &["interrupt", "abort"]),
    ("暂停", &["pause"]),
    ("停止", &["stop"]),
    ("启动", &["start", "launch"]),
    ("初始化", &["init", "initialize"]),
    ("清理", &["cleanup", "clean"]),
    ("释放", &["release", "free"]),
    ("分配", &["allocate"]),
    ("引用", &["reference"]),
    ("指针", &["pointer"]),
    ("借用", &["borrow"]),
    ("所有权", &["ownership"]),
    ("生命周期", &["lifetime"]),
    ("泛型", &["generic"]),
    ("特质", &["trait"]),
    ("关联", &["associated", "assoc"]),
    ("宏", &["macro"]),
    ("派生", &["derive"]),
    ("联合", &["union"]),
    ("元组", &["tuple"]),
    ("哈希表", &["hashmap", "map"]),
    ("堆", &["heap"]),
    ("优先级", &["priority"]),
    ("链表", &["linkedlist", "list"]),
    ("深度", &["depth"]),
    ("广度", &["breadth"]),
    ("宽度", &["width"]),
    ("高度", &["height"]),
    ("坐标", &["coordinate"]),
    ("位置", &["position"]),
    ("偏移", &["offset"]),
    ("尺寸", &["dimension", "size"]),
    ("频率", &["frequency"]),
    ("时间", &["time"]),
    ("日期", &["date"]),
    ("持续时间", &["duration"]),
    ("时间戳", &["timestamp"]),
    ("毫秒", &["millisecond", "ms"]),
    ("时区", &["timezone"]),
    ("输出格式", &["format"]),
    ("解析器", &["parser"]),
    ("索引器", &["indexer"]),
    ("搜索器", &["searcher"]),
    ("写入器", &["writer"]),
    ("读取器", &["reader"]),
    ("分析器", &["analyzer"]),
    ("分词器", &["tokenizer"]),
    ("嵌入模型", &["embedding", "embed"]),
    ("向量库", &["vector", "store"]),
    ("全文搜索", &["fulltext", "bm25", "tantivy"]),
    ("代码块", &["block", "chunk"]),
    ("代码段", &["block", "chunk"]),
    ("文件匹配", &["filematch", "file"]),
    ("搜索结果", &["searchresult", "result"]),
    ("索引文件", &["index", "meta"]),
    ("缓存目录", &["cache", "dir"]),
    ("项目根", &["project_root", "root"]),
    ("自动索引", &["autoindex", "index"]),
    ("增量索引", &["incremental", "index"]),
    ("索引过期", &["stale", "index"]),
    ("跨语言", &["crosslang", "cjk", "semantic"]),
    ("语义检索", &["semantic", "vector", "embedding"]),
    ("向量召回", &["vector", "recall", "search_vector"]),
    ("混合搜索", &["hybrid", "search_hybrid"]),
    ("重排", &["rerank", "rank"]),
    ("聚合", &["aggregate"]),
    ("折叠", &["fold", "aggregate"]),
    ("高亮", &["highlight"]),
    ("上下文行", &["context_lines", "context"]),
    ("令牌预算", &["max_tokens", "token", "budget"]),
    ("输出模式", &["output_mode", "mode"]),
    ("定位模式", &["locate"]),
    ("展开模式", &["expand"]),
    ("文件名搜索", &["filename", "search_filename"]),
    ("全部文件", &["search_all", "all"]),
    ("路径过滤", &["path_filter", "filter"]),
    ("忽略文件", &["gitignore", "ignore"]),
    ("排除模式", &["exclude", "pattern"]),
    ("索引路径", &["index_path", "path"]),
    ("索引状态", &["status"]),
    ("自动重建", &["rebuild", "index"]),
    ("索引版本", &["index_version", "version"]),
    ("系统缓存", &["cache_root", "cache"]),
    ("项目检测", &["detect_project_root", "root"]),
    ("工作区根", &["workspace_root", "root"]),
];

/// 英文停用词 — 符号猜测阶段过滤（"how"/"the" 不是标识符）。
static EN_STOPWORDS: &[&str] = &[
    "the", "a", "an", "of", "to", "for", "in", "on", "at", "by", "with", "without", "and", "or",
    "but", "nor", "so", "yet", "as", "is", "are", "was", "were", "be", "been", "being", "do",
    "does", "did", "have", "has", "had", "can", "could", "should", "would", "may", "might", "must",
    "shall", "will", "not", "no", "yes", "how", "what", "when", "why", "where", "which", "who",
    "whom", "whose", "this", "that", "these", "those", "i", "you", "he", "she", "it", "we", "they",
    "me", "him", "her", "us", "them", "my", "your", "his", "its", "our", "their", "from", "into",
    "onto", "upon", "about", "against", "between", "through", "during", "before", "after", "above",
    "below", "up", "down", "out", "off", "over",
];

/// 提取查询中的 ASCII 词（标识符候选，小写化）。跳过纯数字段。
pub fn ascii_terms(query: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut seen = std::collections::HashSet::new();
    for seg in query.split(|c: char| !c.is_ascii_alphanumeric() && c != '_') {
        if seg.is_empty() {
            continue;
        }
        let t = seg.to_lowercase();
        if t.chars().all(|c| c.is_ascii_digit()) {
            continue; // 纯数字（如 "0 命中" 里的 "0"）不是标识符
        }
        if t.len() > 64 {
            continue;
        }
        if seen.insert(t.clone()) {
            out.push(t);
        }
    }
    out
}

/// 词典映射：查询中包含某中文词 → 收集其英文符号（去重保序）。
pub fn english_synonyms(query: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut seen = std::collections::HashSet::new();
    for (zh, ens) in CODE_DICT {
        if query.contains(zh) {
            for en in *ens {
                if seen.insert(en.to_string()) {
                    out.push(en.to_string());
                }
            }
        }
    }
    out
}

/// 符号猜测词 = ASCII 标识符（去英文停用词）+ 词典映射词，去重保序。
pub fn symbol_terms(query: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut seen = std::collections::HashSet::new();
    for t in ascii_terms(query) {
        if EN_STOPWORDS.contains(&t.as_str()) {
            continue;
        }
        if seen.insert(t.clone()) {
            out.push(t);
        }
    }
    for t in english_synonyms(query) {
        if EN_STOPWORDS.contains(&t.as_str()) {
            continue;
        }
        if seen.insert(t.clone()) {
            out.push(t);
        }
    }
    out
}

/// 0 命中自动重试编排：返回按优先级排序的候选 (阶段, 重试查询)。
///
/// 规则：
/// - 纯 ASCII 查询（无中文）→ BM25 已用原词试过，直接 file 兜底（不重复 BM25）。
/// - 含中文 → ① 英文同义词逐个（先单词后组合）→ ② 符号猜测 → ③ file 兜底。
/// - 候选去重，避免同一查询重复执行。
pub fn retry_plan(query: &str) -> Vec<(RetryKind, String)> {
    let q = query.trim();
    if q.is_empty() {
        return Vec::new();
    }
    if !has_cjk(q) {
        return vec![(RetryKind::FileFallback, q.to_string())];
    }

    let mut out: Vec<(RetryKind, String)> = Vec::new();
    let mut seen_q: std::collections::HashSet<String> = std::collections::HashSet::new();

    // 阶段 1：英文同义词（单词优先，命中即停；组合兜底）
    let syn = english_synonyms(q);
    for w in &syn {
        if seen_q.insert(w.clone()) {
            out.push((RetryKind::EnglishSynonym, w.clone()));
        }
    }
    if syn.len() >= 2 {
        let joined = syn.join(" ");
        if seen_q.insert(joined.clone()) {
            out.push((RetryKind::EnglishSynonym, joined));
        }
    }

    // 阶段 2：符号猜测（ASCII 标识符 + 词典词）
    let syms = symbol_terms(q);
    if !syms.is_empty() {
        let single = syms.first().cloned().unwrap_or_default();
        if !single.is_empty() && seen_q.insert(single.clone()) {
            out.push((RetryKind::SymbolGuess, single));
        }
        if syms.len() >= 2 {
            let joined = syms.join(" ");
            if seen_q.insert(joined.clone()) {
                out.push((RetryKind::SymbolGuess, joined));
            }
        }
    }

    // 阶段 3：file 兜底（文件名/内容子串）
    if seen_q.insert(q.to_string()) {
        out.push((RetryKind::FileFallback, q.to_string()));
    }

    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn english_synonyms_maps_common_terms() {
        let s = english_synonyms("缓存索引");
        assert!(s.contains(&"cache".to_string()), "got: {s:?}");
        assert!(s.contains(&"index".to_string()), "got: {s:?}");
    }

    #[test]
    fn english_synonyms_no_cjk_returns_empty() {
        assert!(english_synonyms("hello world").is_empty());
    }

    #[test]
    fn ascii_terms_extracts_identifiers() {
        let t = ascii_terms("0 命中时如何生成 hint 攻略");
        assert_eq!(t, vec!["hint"]);
    }

    #[test]
    fn ascii_terms_skips_stopwords() {
        // 纯英文停用词不应成为符号猜测候选
        let s = symbol_terms("how to do it");
        assert!(s.is_empty(), "got: {s:?}");
    }

    #[test]
    fn symbol_terms_merges_ascii_and_dict() {
        let s = symbol_terms("hint 缓存");
        assert!(s.contains(&"hint".to_string()), "got: {s:?}");
        assert!(s.contains(&"cache".to_string()), "got: {s:?}");
    }

    #[test]
    fn retry_plan_ascii_only_goes_straight_to_file() {
        let plan = retry_plan("cache_root_xyz");
        assert_eq!(plan.len(), 1);
        assert_eq!(plan[0].0, RetryKind::FileFallback);
    }

    #[test]
    fn retry_plan_cjk_starts_with_english_synonym() {
        let plan = retry_plan("缓存");
        assert_eq!(plan[0].0, RetryKind::EnglishSynonym);
        assert_eq!(plan[0].1, "cache");
        // 最终必有 file 兜底
        assert!(plan.iter().any(|(k, _)| *k == RetryKind::FileFallback));
    }

    #[test]
    fn retry_plan_no_duplicate_queries() {
        let plan = retry_plan("缓存 索引 路由");
        let mut seen = std::collections::HashSet::new();
        for (_, q) in &plan {
            assert!(seen.insert(q.clone()), "duplicate candidate: {q}");
        }
    }

    #[test]
    fn retry_plan_descriptive_chinese_contains_hint_symbol() {
        // 验收案例：描述性中文 + ASCII 词混合查询 → 符号猜测阶段必须含 "hint"
        let plan = retry_plan("0 命中时如何生成中文自救 hint 攻略");
        let qs: Vec<String> = plan.iter().map(|(_, q)| q.clone()).collect();
        assert!(qs.contains(&"hint".to_string()), "got: {qs:?}");
        // 英文同义词阶段应有 hit（"命中"）
        assert!(qs.iter().any(|q| q == "hit"), "got: {qs:?}");
    }
}

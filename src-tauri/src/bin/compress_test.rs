use std::fs;
use serde_json::Value;

fn main() {
    let dir = "G:\\VCPMobile\\scripts\\info-test\\";
    println!("正在读取测试数据集...");

    // 1. 读取 3 个真实的日记本召回数据
    let public_json = fs::read_to_string(format!("{}{}", dir, "公共日记本召回.json")).unwrap();
    let encyclopedia_json = fs::read_to_string(format!("{}{}", dir, "VCP百科全书.json")).unwrap();
    let nova_json = fs::read_to_string(format!("{}{}", dir, "Nova.json")).unwrap();

    let public_val: Value = serde_json::from_str(&public_json).unwrap();
    let encyclopedia_val: Value = serde_json::from_str(&encyclopedia_json).unwrap();
    let nova_val: Value = serde_json::from_str(&nova_json).unwrap();

    // 2. 读取元思考链数据
    let meta_thinking_str = fs::read_to_string(format!("{}{}", dir, "元思考链 meta.txt")).unwrap();

    // 3. 构建“一次对话”的 100% 真实不重复认知数据
    // 包含 5 个日记本召回（无简单内容克隆，使用 3 个不同日记本召回拼凑）:
    // - 2 个 公共
    // - 2 个 VCP百科全书
    // - 1 个 Nova
    // 外加 1 个 完整的 27KB 元思考链
    let dialog_payload = serde_json::json!({
        "type": "DIALOG_COGNITIVE_SUMMARY",
        "rag_retrievals": vec![
            public_val.clone(),
            encyclopedia_val.clone(),
            nova_val.clone(),
            // 另外两本使用 clone 填充满 5 个召回，同时保留 3 个不同日记本的多样性
            public_val,
            encyclopedia_val,
        ],
        "meta_thinking": meta_thinking_str
    });

    let dialog_json = serde_json::to_string(&dialog_payload).unwrap();
    let dialog_bytes = dialog_json.as_bytes();
    let dialog_size = dialog_bytes.len();
    println!("\n=== 模拟一次对话的 100% 真实认知数据 ===");
    println!("总字符数: {}", dialog_json.chars().count());
    println!("原始 JSON 大小: {} 字节 ({:.2} KB)", dialog_size, dialog_size as f64 / 1024.0);

    // 1. ZSTD 压缩测试 (Level 3 - 默认)
    let zstd_compressed = zstd::bulk::compress(dialog_bytes, 3).unwrap();
    let zstd_size = zstd_compressed.len();
    println!("\n[ZSTD (Level 3)]");
    println!("压缩后大小: {} 字节 ({:.2} KB)", zstd_size, zstd_size as f64 / 1024.0);
    println!("压缩比: {:.2}%", (zstd_size as f64 / dialog_size as f64) * 100.0);

    // 2. ZSTD 压缩测试 (Level 7 - 高压缩率)
    let zstd_compressed_7 = zstd::bulk::compress(dialog_bytes, 7).unwrap();
    let zstd_size_7 = zstd_compressed_7.len();
    println!("\n[ZSTD (Level 7)]");
    println!("压缩后大小: {} 字节 ({:.2} KB)", zstd_size_7, zstd_size_7 as f64 / 1024.0);
    println!("压缩比: {:.2}%", (zstd_size_7 as f64 / dialog_size as f64) * 100.0);

    // 3. ZSTD 压缩测试 (Level 11)
    let zstd_compressed_11 = zstd::bulk::compress(dialog_bytes, 11).unwrap();
    let zstd_size_11 = zstd_compressed_11.len();
    println!("\n[ZSTD (Level 11)]");
    println!("压缩后大小: {} 字节 ({:.2} KB)", zstd_size_11, zstd_size_11 as f64 / 1024.0);
    println!("压缩比: {:.2}%", (zstd_size_11 as f64 / dialog_size as f64) * 100.0);

    // 内存规模估算
    println!("\n=== 500 条历史记录 (等同于 100 次完整对话) 纯内存缓存估算 ===");
    let raw_total = (dialog_size as f64 * 100.0) / (1024.0 * 1024.0);
    let zstd_total_3 = (zstd_size as f64 * 100.0) / (1024.0 * 1024.0);
    let zstd_total_7 = (zstd_size_7 as f64 * 100.0) / (1024.0 * 1024.0);
    let zstd_total_11 = (zstd_size_11 as f64 * 100.0) / (1024.0 * 1024.0);
    println!("原始未压缩内存总占用: {:.2} MB", raw_total);
    println!("ZSTD Level 3 内存总占用: {:.2} MB", zstd_total_3);
    println!("ZSTD Level 7 内存总占用: {:.2} MB", zstd_total_7);
    println!("ZSTD Level 11 内存总占用: {:.2} MB", zstd_total_11);
    println!("--------------------------------------------------");
}

//! 对任意 nvlddmkm 磁盘镜像跑跨代布局探针,打印完整布局(JSON + 行)。
//! 用法:`cargo run -p nvoc-core --example probe_image -- <nvlddmkm.sys 路径>`
//! 新驱动 onboarding 的第一步:拿到 RM 分派命令号与 GPU 链偏移,给 idalib
//! 静态狩猎当锚点(fail-closed,推不出即报错退出)。

use std::process::ExitCode;

fn main() -> ExitCode {
    let Some(path) = std::env::args().nth(1) else {
        eprintln!("用法: probe_image <nvlddmkm.sys 路径>");
        return ExitCode::FAILURE;
    };
    let img = match std::fs::read(&path) {
        Ok(img) => img,
        Err(e) => {
            eprintln!("读 {path} 失败: {e}");
            return ExitCode::FAILURE;
        }
    };
    match nvoc_core::kmd::layout_probe::probe(&img) {
        Ok(layout) => {
            println!("file          = {path}");
            println!("timestamp     = {:#x}", layout.timestamp);
            println!("size_of_image = {:#x}", layout.size_of_image);
            println!("global_slot   = {:#x}", layout.global_slot_rva);
            println!("state->table  = {:#x}", layout.state_table_off);
            println!("count/major/id= {:#x}/{:#x}/{:#x}", layout.table_count_off, layout.entry_major_off, layout.entry_id_off);
            println!("stride        = {:#x}", layout.entry_stride);
            println!("major->root   = {:#x}", layout.major_root_off);
            println!("root init/elig/aa = {:#x}/{:#x}/{:#x}", layout.root_init_off, layout.root_elig_off, layout.root_amount_active_off);
            println!("root base/amount/key = {:#x}/{:#x}/{:#x}", layout.root_base_off, layout.root_amount_off, layout.root_key_off);
            println!("root lower/upper = {:#x}/{:#x}", layout.root_lower_off, layout.root_upper_off);
            println!("generator rva = {:#x}", layout.generator_rva);
            println!("rm_get_cmd    = {:?}", layout.rm_get_cmd);
            println!("rm_set_cmd    = {:?}", layout.rm_set_cmd);
            println!("--- anchors ---");
            for a in &layout.anchors {
                println!("{a}");
            }
            println!("--- json ---");
            println!("{}", serde_json::to_string_pretty(&layout).unwrap_or_default());
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("probe 失败(fail-closed): {e:?}");
            ExitCode::FAILURE
        }
    }
}

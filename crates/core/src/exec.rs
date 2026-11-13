//! 被连方：在本机跑一条命令，不开终端，输出分标准输出和标准错误两路回传，最后给退出码。
//!
//! 和终端的区别：没有伪终端，命令的输入是空的，程序不会因为“连着终端”而换一副样子（颜色、分页），
//! 输出能原样拿回去。对面中途取消或走了，整组进程一起结束；命令自己放到后台的进程不管，
//! 和 ssh 一样，要长期跑的东西自己用 nohup 并重定向输出。

/// 输出帧第一个字节：标准输出
pub const STDOUT: u8 = 1;
/// 输出帧第一个字节：标准错误
pub const STDERR: u8 = 2;
/// 命令最长多少字节，防对面塞超大的东西
pub const MAX_COMMAND_LEN: usize = 256 * 1024;
#[cfg(windows)]
pub(crate) const CREATE_NO_WINDOW: u32 = 0x08000000;

// 起进程跑命令的那部分只有被连的一方用，手机端（关掉 host）不编
#[cfg(feature = "host")]
mod run;
#[cfg(feature = "host")]
pub use run::serve;
#[cfg(feature = "host")]
pub(crate) use run::{AbortOnDrop, Group};

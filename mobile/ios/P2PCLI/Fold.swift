import SwiftUI

/// iPhone Duo 的折痕。半折时（像书一样拿着，或者像笔记本那样立在桌上）系统把折痕报成一块“分隔”的保留区域；
/// 全展开、合上时这块区域不生效。系统的列表、分栏、弹窗会自己躲开它，
/// 终端和按键条是自己画的，按苹果的说明用 GeometryProxy.reservedRegions 查到折痕后自己让开。
/// 只有 iOS 27.1 以上有这个接口；别的设备上一直是空
enum Fold {
    /// 横着的折痕（像笔记本那样立着）：在这个视图里的位置。太靠边的不算。
    /// 量的视图要忽略键盘（ignoresSafeArea(.keyboard)），否则键盘弹起来以后视图只剩上半截，折痕就量不到了
    static func horizontal(_ geo: GeometryProxy) -> CGRect? {
        #if DEBUG
        // 模拟器的折叠姿态只能在 Device Hub 里手动拨；界面测试用这个启动参数假装半折
        if ProcessInfo.processInfo.arguments.contains("-P2PCLIFakeFold"), geo.size.height > 400 {
            let y = (geo.size.height * 0.5).rounded()
            return CGRect(x: 0, y: y - 12, width: geo.size.width, height: 24)
        }
        #endif
        guard #available(iOS 27.1, *) else { return nil }
        for r in geo.reservedRegions(kind: .division) where r.isActive && r.frame.width > r.frame.height {
            if r.frame.minY > 120 && r.frame.maxY < geo.size.height - 60 { return r.frame }
        }
        return nil
    }

    /// 竖着的折痕（像书一样拿着）：在这个视图里横向占的范围
    static func vertical(_ geo: GeometryProxy) -> ClosedRange<CGFloat>? {
        guard #available(iOS 27.1, *) else { return nil }
        for r in geo.reservedRegions(kind: .division) where r.isActive && r.frame.height >= r.frame.width {
            let lo = max(0, r.frame.minX), hi = min(geo.size.width, r.frame.maxX)
            if lo > 60 && hi < geo.size.width - 60 && hi >= lo { return lo...hi }
        }
        return nil
    }
}

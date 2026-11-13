import { common } from '@kit.AbilityKit';
import { deviceInfo } from '@kit.BasicServicesKit';
import p2pcli from 'libp2pcli.so';
import { P2p } from 'libp2pcli.so';
import { DeviceView, MobileConfig, Overview, parseError } from '../common/Model';

// 第一版：启动 Rust 那边、列出设备，先把 ArkTS → Rust 这条路打通
@Entry
@Component
struct Index {
  @State status: string = '正在启动…';
  @State devices: DeviceView[] = [];
  private p2p?: P2p;

  aboutToAppear(): void {
    const ctx = AppStorage.get<common.UIAbilityContext>('context') as common.UIAbilityContext;
    const config: MobileConfig = {
      data_dir: ctx.filesDir + '/p2pcli',
      inbox_dir: ctx.filesDir + '/inbox',
      device_name: deviceInfo.marketName,
      server: null,
      relay: null,
      proxy: null,
      app_version: null,
      // Rust 编译目标的系统名是 linux，这里明确告诉对方是鸿蒙
      platform: 'harmonyos'
    };
    p2pcli.start(JSON.stringify(config), (event: string) => {
      this.refresh();
    }).then((p: P2p) => {
      this.p2p = p;
      this.refresh();
    }).catch((e: Error) => {
      this.status = '启动失败：' + parseError(e).detail;
    });
  }

  refresh(): void {
    if (!this.p2p) {
      return;
    }
    const ov = JSON.parse(this.p2p.overview()) as Overview;
    this.devices = ov.devices.concat(ov.nearby);
    this.status = `${ov.name} · 牵线服务器 ${ov.server.state} · ${this.devices.length} 台设备`;
  }

  build() {
    Column({ space: 12 }) {
      Text('P2PCLI').fontSize(28).fontWeight(FontWeight.Bold).fontColor('#FCFCFC')
      Text(this.status).fontSize(14).fontColor('#9F9FA9')
      List({ space: 8 }) {
        ForEach(this.devices, (d: DeviceView) => {
          ListItem() {
            Column({ space: 4 }) {
              Text(d.remark ?? d.name).fontSize(16).fontColor('#FCFCFC')
              Text(`${d.platform} · ${d.presence}${d.lan_online ? ' · 局域网' : ''}`).fontSize(13).fontColor('#9F9FA9')
            }
            .alignItems(HorizontalAlign.Start)
            .width('100%')
            .padding(14)
            .backgroundColor('#18181B')
            .borderRadius(16)
          }
        }, (d: DeviceView) => d.endpoint_id)
      }
      .layoutWeight(1)
    }
    .padding({ left: 16, right: 16, top: 48 })
    .width('100%')
    .height('100%')
    .backgroundColor('#060607')
    .alignItems(HorizontalAlign.Start)
  }
}

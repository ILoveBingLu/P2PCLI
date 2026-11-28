//! 测本机 IPv4 NAT：锥形还是对称、开端口的规律、放哪些包进来，以及按顺序开时会猜哪些端口。
//! cargo run -p p2pcli-core --example nat_probe
#[tokio::main]
async fn main() {
    match p2pcli_core::diag::stun_probe(p2pcli_core::diag::STUN_SERVER).await {
        Ok(s) => println!("类型：{:?}（本机 {}，外网 {}，第二个目标看到 {:?}）", s.nat, s.local, s.mapped, s.mapped_second),
        Err(e) => println!("类型测不出：{e:#}"),
    }
    for round in 1..=2 {
        match p2pcli_core::natpredict::probe().await {
            Ok(p) => {
                println!("第 {round} 次：开端口 {:?}，依次看到 {:?}（本机端口 {}），放包 {:?}", p.allocation, p.ports, p.local_port, p.filtering);
                if let Some(last) = p.last_port() {
                    let g = p2pcli_core::natpredict::guesses(&p, &[std::net::SocketAddrV4::new(p.public_ip, last)]);
                    println!("  会猜的端口：{}", g.iter().map(|a| a.port().to_string()).collect::<Vec<_>>().join(" "));
                }
            }
            Err(e) => println!("第 {round} 次：测不出规律：{e:#}"),
        }
    }
}

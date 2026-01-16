use std::{process::exit, time::Duration};

use cobs::CobsDecoderOwned;
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt, stderr},
    net::{TcpListener, TcpStream},
    process::Command,
    time::sleep,
};
use vex_v5_serial::{
    Connection,
    serial::{self, SerialDevice},
};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let mut args = std::env::args().skip(1);

    let Some(elf_file) = args.next() else {
        eprintln!("Usage: v5gdb <PATH-TO-ELF-FILE-TO-DEBUG>");
        exit(1);
    };

    let mut all_devices = serial::find_devices()?;
    if all_devices.is_empty() {
        eprintln!("No V5 device is connected");
        exit(1);
    }
    let device = all_devices.remove(0);

    let server = TcpListener::bind("127.0.0.1:35537").await?;
    tokio::spawn(async move { serve_device_serial(device, server).await.unwrap() });

    let mut gdb = Command::new("gdb");
    gdb.arg(format!("--eval-command=file {elf_file}"));
    gdb.arg("--eval-command=target remote :35537");
    let mut process = gdb.spawn()?;
    process.wait().await?;

    Ok(())
}

async fn serve_device_serial(device: SerialDevice, server: TcpListener) -> anyhow::Result<()> {
    let mut connection = device.connect(Duration::from_secs(5))?;

    let mut program_output = [0; 2048];
    let mut program_input = [0; 4096];

    let mut decoder = CobsDecoderOwned::new(2048);

    let (mut conn, _addr) = server.accept().await?;

    loop {
        tokio::select! {
            read = connection.read_user(&mut program_output) => {
                if let Ok(size) = read {
                    let mut incoming_bytes = &program_output[..size];

                    while !incoming_bytes.is_empty() {
                        match decoder.push(&*incoming_bytes) {
                            Ok(Some(report)) => {
                                incoming_bytes = &incoming_bytes[report.parsed_size()..];
                                let packet = &decoder.dest()[..report.frame_size()];
                                handle_packet(&mut conn, packet).await?;
                                decoder.reset();
                            }
                            Err(_) => {
                                eprintln!("[COBS decode failed]");
                                decoder.reset();
                            },
                            _ => {}
                        }
                    }
                }
            },
            read = conn.read(&mut program_input) => {
                match read {
                    Ok(0) => break,
                    Ok(size) => {
                        connection.write_user(&program_input[..size]).await.unwrap();
                    }
                    _ => {}
                }
            }
        }

        sleep(Duration::from_millis(10)).await;
    }

    Ok(())
}

async fn handle_packet(writer: &mut TcpStream, packet: &[u8]) -> anyhow::Result<()> {
    let Some(channel_byte) = packet.first() else {
        eprintln!("[Missing channel]");
        return Ok(());
    };

    let body = &packet[1..];

    match channel_byte {
        b'u' => {
            stderr().write_all(body).await?;
        }
        b'd' => {
            writer.write_all(body).await?;
        }
        _ => eprintln!("[Bad channel]"),
    }

    Ok(())
}

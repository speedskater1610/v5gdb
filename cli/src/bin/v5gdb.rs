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

    let known_gdb_names = ["arm-none-eabi-gdb", "gdb-multiarch", "gdb"];

    let mut gdb = None;
    for name in known_gdb_names {
        let mut cmd = Command::new(name);
        cmd.arg(format!("--eval-command=file {elf_file}"));
        cmd.arg("--eval-command=target remote :35537");

        if let Ok(child) = cmd.spawn() {
            gdb = Some(child);
        }
    }

    let Some(mut gdb) = gdb else {
        eprintln!("Error: One of the following GDB executables must be installed.");
        eprintln!("{:?}", known_gdb_names);
        exit(1);
    };

    gdb.wait().await?;

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
                        // If we receive any invalid packets, they might be prints from the user, so
                        // we should print them out as-is.

                        match decoder.push(incoming_bytes) {
                            Ok(Some(report)) => {
                                let packet = &decoder.dest()[..report.frame_size()];
                                let was_valid = handle_packet(&mut conn, packet).await;

                                if !was_valid {
                                    // If we receive an invalid packet, fall back to assuming it's
                                    // just raw data and print it out.
                                    stderr().write_all(&incoming_bytes[..report.parsed_size()]).await?;
                                }

                                decoder.reset();
                                incoming_bytes = &incoming_bytes[report.parsed_size()..];
                            }
                            Err(_) => {
                                // We are only discarding one byte, so print that one.
                                let invalid_byte = incoming_bytes[0];
                                stderr().write_all(&[invalid_byte]).await?;

                                // Skip one byte and try to resynchronize
                                incoming_bytes = &incoming_bytes[1..];
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

/// Handles a packet from the V5 device and returns whether it was valid.
async fn handle_packet(writer: &mut TcpStream, packet: &[u8]) -> bool {
    let Some(channel_byte) = packet.first() else {
        // Missing channel
        return false;
    };

    let body = &packet[1..];

    match channel_byte {
        b'u' => {
            _ = stderr().write_all(body).await;
        }
        b'd' => {
            _ = writer.write_all(body).await;
        }
        // Unknown channel
        _ => return false,
    }

    true
}

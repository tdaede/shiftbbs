extern crate encoding_rs;
extern crate crc;
extern crate serde;
extern crate toml;
use encoding_rs::SHIFT_JIS;
use serde::Deserialize;

use std::io::prelude::*;
use std::io::{BufReader, BufWriter};
use std::net::{TcpListener, TcpStream};
use std::fs;
use std::path::PathBuf;
use std::thread;
use std::time::Duration;
use crc::{Crc, CRC_16_XMODEM};

pub const CRC_XMODEM: Crc<u16> = Crc::<u16>::new(&CRC_16_XMODEM);

struct Client {
    stream: TcpStream,
    rb: BufReader<TcpStream>,
    wb: BufWriter<TcpStream>
}

impl Client {
    fn new(stream: TcpStream) -> Client {
        stream.set_read_timeout(Some(Duration::from_secs(10))).unwrap();
        let rb = BufReader::new(stream.try_clone().unwrap());
        let wb = BufWriter::new(stream.try_clone().unwrap());
        return Client { stream, rb, wb };
    }
    fn write_str(&mut self, s: &str) {
        let (encoded, _, _) = SHIFT_JIS.encode(s);
        self.stream.write_all(&encoded).unwrap();
    }
    fn write_line(&mut self, s: &str) {
        self.write_str(s);
        self.write_str("\r");
    }
    fn _pause_newline(&mut self) {
        let mut b = String::new();
        self.rb.read_line(&mut b).unwrap();
    }
    fn _echo_infinite(&mut self) {
        loop {
            let mut b = [0; 1];
            self.rb.read_exact(&mut b).unwrap();
            self.stream.write_all(&b).unwrap();
        }
    }
    // TODO: make this not hang indefinitely on loss of connection
    // this actually returns a string but we might need to change that
    // as arrow keys etc aren't sjis strings
    fn get_key(&mut self) -> String {
        let mut b = [0; 1];
        loop {
            if let Ok(()) = self.rb.read_exact(&mut b) {
                let (decoded, _, _) = SHIFT_JIS.decode(&b);
                return decoded.to_string();
            }
        }
    }
    fn get_byte(&mut self) -> Result<u8, std::io::Error> {
        let mut b = [0; 1];
        self.rb.read_exact(&mut b)?;
        Ok(b[0])
    }
    fn readline(&mut self) -> Result<String, std::io::Error> {
        let mut s = String::new();
        loop {
            let k = self.get_key();
            self.write_str(&k);
            self.wb.flush()?;
            if k == "\r" {
                return Ok(String::from(s.trim()));
            }
            s.push_str(&k);
        }
    }
}

fn file_list(c: &mut Client) {
    c.write_line("File list:");
    c.write_line("≡≡≡≡≡≡≡≡≡≡");
    let paths = fs::read_dir("./files/").unwrap();
    for path in paths {
        c.write_line(&path.unwrap().file_name().into_string().unwrap());
    }
}

fn xmodem_send_packet_crc(n: u8, d: &[u8; 128], c: &mut Client) {
    println!("sending xmodem packet");
    let checksum = CRC_XMODEM.checksum(d);
    loop {
        c.wb.write_all(&[0x01]).unwrap();
        c.wb.write_all(&[n, 0xFF-n]).unwrap();
        c.wb.write_all(d).unwrap();
        c.wb.write_all(&checksum.to_be_bytes()).unwrap();
        c.wb.flush().unwrap();
        if c.get_byte().unwrap() == 0x06 {
            println!("got ack");
            break;
        }
        println!("probably got nack");
    }
}

fn xmodem_send_packet_crc_1k(n: u8, d: &[u8; 1024], c: &mut Client) {
    println!("sending xmodem packet");
    let checksum = CRC_XMODEM.checksum(d);
    loop {
        c.wb.write_all(&[0x02]).unwrap();
        c.wb.write_all(&[n, 0xFF-n]).unwrap();
        c.wb.write_all(d).unwrap();
        c.wb.write_all(&checksum.to_be_bytes()).unwrap();
        c.wb.flush().unwrap();
        if c.get_byte().unwrap() == 0x06 {
            println!("got ack");
            break;
        }
        println!("probably got nack");
    }
}

fn xmodem_receive_packet_crc(c: &mut Client) -> Result<Vec<u8>,std::io::Error> {
    println!("receiving xmodem packet");
    loop {
        let start_byte = c.get_byte()?;
        if start_byte == 0x04 {
            println!("got end of transmission");
            c.wb.write_all(&[0x06])?;
            c.wb.flush()?;
            return Ok(vec![0; 0]); // indicates an end of the current file
        }
        if (start_byte != 0x01) && (start_byte != 0x02) {
            println!("got bad packet start byte");
            c.wb.write_all(&[0x15])?;
            c.wb.flush()?;
            continue;
        }
        let n = c.get_byte()?;
        let n2 = c.get_byte()?;
        if n.wrapping_add(n2) != 0xFF {
            println!("mismatched packet numbers");
            c.wb.write_all(&[0x015])?;
            c.wb.flush()?;
            continue;
        }
        let mut data = match start_byte {
            0x01 => vec![0; 128],
            0x02 => vec![0; 1024],
            _ => vec![0; 0]
        };
        c.rb.read_exact(&mut data)?;
        let checksum = CRC_XMODEM.checksum(&data);
        let mut recv_checksum = [0; 2];
        c.rb.read_exact(&mut recv_checksum)?;
        if recv_checksum != checksum.to_be_bytes() {
            c.wb.write_all(&[0x15])?;
            c.wb.flush()?;
            println!("failed checksum");
            continue;
        }
        c.wb.write_all(&[0x06])?; // ack
        println!("sent packet ack");
        c.wb.flush()?;
        return Ok(data);
    }
}

fn _xmodem_send(c: &mut Client) {
    let mut file = fs::File::open("./files/mdiskv3.xdf").unwrap();
    let b = c.get_byte().unwrap();
    match b {
        0x43 => {
            println!("got request for XMODEM-CRC");
            let mut packet_no: u8 = 1;
            loop {
                let mut chunk = [0x26; 128];
                let mut bytes_read = 0;
                loop {
                    let a = file.read(&mut chunk[bytes_read..]).unwrap();
                    bytes_read += a;
                    if bytes_read >= 128 { break; }
                    if a == 0 { break; }
                };
                xmodem_send_packet_crc(packet_no, &chunk, c);
                packet_no = packet_no.wrapping_add(1);
                if bytes_read < 128 { break; }
            }
            c.wb.write_all(&[0x04]).unwrap();
            c.wb.flush().unwrap();
            let _ = c.get_byte(); // consume last ack
        }
        _ => {
            // don't know how to handle these
            eprintln!("Got unrecognized transfer type during xmodem transfer: {:02x}", b);
            c.write_line(&format!("Got unrecognized transfer type {:02x}, only XMODEM-CRC is currently supported.", b));
        }
    }
}

fn ymodem_send(c: &mut Client, path: &PathBuf) -> Result<(), std::io::Error> {
    let mut file = fs::File::open(path)?;
    loop {
        let b = c.get_byte()?;
        match b {
            0x43 => {
                println!("got request for XMODEM-CRC");
                let (encoded_filename, _, _) = SHIFT_JIS.encode(&path.file_name().unwrap().to_str().unwrap());
                let mut first_chunk = Vec::new();
                first_chunk.write(&encoded_filename).unwrap();
                first_chunk.write(&[0]).unwrap();
                first_chunk.write(&vec![0; 1024 - first_chunk.len()]).unwrap();
                xmodem_send_packet_crc_1k(0, &first_chunk[0..1024].try_into().unwrap(), c);
                let mut packet_no: u8 = 1;
                loop {
                    let mut chunk = [0x26; 1024];
                    let mut bytes_read = 0;
                    loop {
                        let a = file.read(&mut chunk[bytes_read..]).unwrap();
                        bytes_read += a;
                        if bytes_read >= 1024 { break; }
                        if a == 0 { break; }
                    };
                    xmodem_send_packet_crc_1k(packet_no, &chunk, c);
                    packet_no = packet_no.wrapping_add(1);
                    if bytes_read < 1024 { break; }
                }
                c.wb.write_all(&[0x04]).unwrap();
                c.wb.flush().unwrap();
                let _ = c.get_byte().unwrap(); // consume last ack
                // send end of files
                xmodem_send_packet_crc(0, &[0; 128], c);
                c.wb.flush().unwrap();
                return Ok(());
            }
            0x0a => {},
            0x0d => {}, // eat any garbage new lines
            _ => {
                // don't know how to handle these
                eprintln!("Got unrecognized transfer type during xmodem transfer: {:02x}", b);
                c.write_line(&format!("Got unrecognized transfer type {:02x}, only XMODEM-CRC is currently supported.", b));
                break;
            }
        }
    }
    Err(std::io::Error::from(std::io::ErrorKind::Other))
}

fn ymodem_receive(c: &mut Client, path: &PathBuf) -> Result<(), std::io::Error> {
    loop {
        c.wb.write_all(&[0x43])?;
        c.wb.flush()?;
        if let Ok(header_packet) = xmodem_receive_packet_crc(c) {
            if header_packet[0] == 0 {
                // end of files
                return Ok(());
            }
            let mut filename_bytes = Vec::new();
            for b in header_packet {
                if b == 0 {
                    break;
                }
                filename_bytes.push(b);
            }
            let mut file_path = path.clone();
            let (decoded_filename, _, _) = SHIFT_JIS.decode(&filename_bytes);
            file_path.push(decoded_filename.to_string());
            if !file_path.canonicalize()?.starts_with(path.canonicalize()?) {
                return Err(std::io::Error::from(std::io::ErrorKind::Other));
            }
            let mut file = std::fs::File::create(file_path)?;
            loop {
                c.wb.write_all(&[0x43])?;
                c.wb.flush()?;
                let data = xmodem_receive_packet_crc(c)?;
                if data.len() == 0 {
                    break; // finished file
                }
                file.write_all(&data)?;
            }
        }
    }
}

fn download_file(c: &mut Client) {
    c.write_line("Please enter file name to download:");
    let filename = c.readline().unwrap();
    c.write_line("Downloading file!");
    c.write_line("Please enable YMODEM file download now (muterm: F5)");
    ymodem_send(c, &PathBuf::from("./files/").join(&filename)).unwrap();
}

fn upload_files(c: &mut Client) {
    c.write_line("Uploading files!");
    c.write_line("Please trigger YMODEM file upload now (muterm: F4)");
    c.write_line("You will have to wait up to 10 seconds for the transfer to start.");
    ymodem_receive(c, &PathBuf::from("./files/")).unwrap();
}

fn handle_client(config: Config, stream: TcpStream) {
    let mut c = Client::new(stream);
    c.write_line("Enter system password:");
    let read_password = c.readline().unwrap();
    if read_password != config.system_password {
        c.write_line("Wrong system password!");
        return;
    }
    c.write_line("Welcome to ShiftBBS!");
    loop {
        c.write_line("Main menu:");
        c.write_line("l: list files");
        c.write_line("d: download file");
        c.write_line("u: upload files");
        c.write_line("q: quit");
        let key = c.get_key();
        match key.as_str() {
            "l" => {
                file_list(&mut c);
            },
            "q" => {
                c.write_line("Bye!");
                break;
            },
            "d" => {
                download_file(&mut c);
            },
            "u" => {
                upload_files(&mut c);
            }
            _ => {
                // just do nothing
            }
        }
    }
}

#[derive(Deserialize, Clone)]
struct Config {
    system_password: String
}

fn main() -> std::io::Result<()> {
    let mut config_str = String::new();
    std::fs::File::open("config.toml")?.read_to_string(&mut config_str)?;
    let config: Config = toml::from_str(&config_str)?;

    let listener = TcpListener::bind("0.0.0.0:6800")?;

    // accept connections and process them serially
    for stream in listener.incoming() {
        let config = config.clone();
        thread::spawn(|| {
            handle_client(config, stream.unwrap());
        });
    }
    Ok(())
}

use byteorder::{LittleEndian, ReadBytesExt, WriteBytesExt};
use std::io::Cursor;

fn main() {
    let mut data: Vec<u8> = Vec::new();

    // push source "system"
    data.write_u64::<LittleEndian>(6).unwrap();
    data.extend_from_slice(b"system");

    // push id 2000
    data.write_u32::<LittleEndian>(2000).unwrap();

    // push job_id 5678
    data.write_u32::<LittleEndian>(5678).unwrap();

    let hash = "71eddf90-9de5-4dff-a307-b2d2eb743d81";
    data.write_u64::<LittleEndian>(hash.len() as u64).unwrap();
    data.extend_from_slice(hash.as_bytes());

    // pop source
    let mut index = 0;
    let mut rdr = Cursor::new(&data[index..]);
    let len = rdr.read_u64::<LittleEndian>().unwrap() as usize;
    index += 8;
    let source = String::from_utf8(data[index..index + len].to_vec()).unwrap();
    index += len;

    // pop id
    let mut rdr = Cursor::new(&data[index..]);
    let id = rdr.read_u32::<LittleEndian>().unwrap();
    index += 4;

    // pop job_id
    let mut rdr = Cursor::new(&data[index..]);
    let job_id = rdr.read_u32::<LittleEndian>().unwrap();
    index += 4;

    // pop hash
    let mut rdr = Cursor::new(&data[index..]);
    let hash_len = rdr.read_u64::<LittleEndian>().unwrap() as usize;
    index += 8;
    let hash_popped = String::from_utf8(data[index..index + hash_len].to_vec()).unwrap();

    println!(
        "source: {}, id: {}, job_id: {}, hash: {}, hash_len: {}",
        source, id, job_id, hash_popped, hash_len
    );
}

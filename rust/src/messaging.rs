use byteorder::{LittleEndian, ReadBytesExt, WriteBytesExt};
use std::io::Cursor;

pub const SYSTEM_SOURCE: &str = "system";

pub const SERVER_READY: u32 = 1000;

pub const SUBMIT_JOB: u32 = 2000;
pub const UPDATE_JOB: u32 = 2001;
pub const CANCEL_JOB: u32 = 2002;
pub const DELETE_JOB: u32 = 2003;

pub const FILE_DOWNLOAD: u32 = 4000;
pub const FILE_DOWNLOAD_DETAILS: u32 = 4001;
pub const FILE_DOWNLOAD_ERROR: u32 = 4002;
pub const FILE_CHUNK: u32 = 4003;
pub const PAUSE_FILE_CHUNK_STREAM: u32 = 4004;
pub const RESUME_FILE_CHUNK_STREAM: u32 = 4005;
pub const FILE_LIST: u32 = 4006;
pub const FILE_LIST_ERROR: u32 = 4007;
pub const UPLOAD_FILE: u32 = 4500;
pub const FILE_UPLOAD_CHUNK: u32 = 4501;
pub const FILE_UPLOAD_ERROR: u32 = 4502;
pub const FILE_UPLOAD_COMPLETE: u32 = 4503;

pub const DB_JOB_GET_BY_JOB_ID: u32 = 5000;
pub const DB_JOB_GET_BY_ID: u32 = 5001;
pub const DB_JOB_GET_RUNNING_JOBS: u32 = 5002;
pub const DB_JOB_DELETE: u32 = 5003;
pub const DB_JOB_SAVE: u32 = 5004;

pub const DB_JOBSTATUS_GET_BY_JOB_ID_AND_WHAT: u32 = 6000;
pub const DB_JOBSTATUS_GET_BY_JOB_ID: u32 = 6001;
pub const DB_JOBSTATUS_DELETE_BY_ID_LIST: u32 = 6002;
pub const DB_JOBSTATUS_SAVE: u32 = 6003;

pub const DB_RESPONSE: u32 = 7000;

pub const DB_BUNDLE_CREATE_OR_UPDATE_JOB: u32 = 8000;
pub const DB_BUNDLE_GET_JOB_BY_ID: u32 = 8001;
pub const DB_BUNDLE_DELETE_JOB: u32 = 8002;

pub const PENDING: u32 = 10;
pub const SUBMITTING: u32 = 20;
pub const SUBMITTED: u32 = 30;
pub const QUEUED: u32 = 40;
pub const RUNNING: u32 = 50;
pub const CANCELLING: u32 = 60;
pub const CANCELLED: u32 = 70;
pub const DELETING: u32 = 80;
pub const DELETED: u32 = 90;
pub const ERROR: u32 = 400;
pub const WALL_TIME_EXCEEDED: u32 = 401;
pub const OUT_OF_MEMORY: u32 = 402;
pub const COMPLETED: u32 = 500;

#[repr(u32)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Priority {
    Highest = 0,
    Medium = 10,
    Lowest = 19,
}

pub struct Message {
    pub id: u32,
    pub source: String,
    pub priority: Priority,
    data: Vec<u8>,
    index: usize,
}

impl Message {
    pub fn new(id: u32, priority: Priority, source: &str) -> Self {
        let mut msg = Message {
            id,
            source: source.to_string(),
            priority,
            data: Vec::new(),
            index: 0,
        };

        // Push the source
        msg.push_string(source);

        // Push the id
        msg.push_uint(id);

        msg
    }

    pub fn from_data(vdata: Vec<u8>) -> Self {
        let mut msg = Message {
            id: 0,
            source: String::new(),
            priority: Priority::Lowest,
            data: vdata,
            index: 0,
        };

        // Parsing should match the C++ constructor (which pushes source then id)
        msg.source = msg.pop_string();
        msg.id = msg.pop_uint();

        msg
    }

    pub fn push_bool(&mut self, value: bool) {
        self.push_ubyte(if value { 1 } else { 0 });
    }

    pub fn pop_bool(&mut self) -> bool {
        self.pop_ubyte() == 1
    }

    pub fn push_ubyte(&mut self, value: u8) {
        self.data.write_u8(value).unwrap();
    }

    pub fn pop_ubyte(&mut self) -> u8 {
        let mut rdr = Cursor::new(&self.data[self.index..]);
        let value = rdr.read_u8().unwrap();
        self.index += 1;
        value
    }

    pub fn push_byte(&mut self, value: i8) {
        self.data.write_i8(value).unwrap();
    }

    pub fn pop_byte(&mut self) -> i8 {
        let mut rdr = Cursor::new(&self.data[self.index..]);
        let value = rdr.read_i8().unwrap();
        self.index += 1;
        value
    }

    pub fn push_ushort(&mut self, value: u16) {
        self.data.write_u16::<LittleEndian>(value).unwrap();
    }

    pub fn pop_ushort(&mut self) -> u16 {
        let mut rdr = Cursor::new(&self.data[self.index..]);
        let value = rdr.read_u16::<LittleEndian>().unwrap();
        self.index += 2;
        value
    }

    pub fn push_short(&mut self, value: i16) {
        self.data.write_i16::<LittleEndian>(value).unwrap();
    }

    pub fn pop_short(&mut self) -> i16 {
        let mut rdr = Cursor::new(&self.data[self.index..]);
        let value = rdr.read_i16::<LittleEndian>().unwrap();
        self.index += 2;
        value
    }

    pub fn push_uint(&mut self, value: u32) {
        self.data.write_u32::<LittleEndian>(value).unwrap();
    }

    pub fn pop_uint(&mut self) -> u32 {
        let mut rdr = Cursor::new(&self.data[self.index..]);
        let value = rdr.read_u32::<LittleEndian>().unwrap();
        self.index += 4;
        value
    }

    pub fn push_int(&mut self, value: i32) {
        self.data.write_i32::<LittleEndian>(value).unwrap();
    }

    pub fn pop_int(&mut self) -> i32 {
        let mut rdr = Cursor::new(&self.data[self.index..]);
        let value = rdr.read_i32::<LittleEndian>().unwrap();
        self.index += 4;
        value
    }

    pub fn push_ulong(&mut self, value: u64) {
        self.data.write_u64::<LittleEndian>(value).unwrap();
    }

    pub fn pop_ulong(&mut self) -> u64 {
        let mut rdr = Cursor::new(&self.data[self.index..]);
        let value = rdr.read_u64::<LittleEndian>().unwrap();
        self.index += 8;
        value
    }

    pub fn push_long(&mut self, value: i64) {
        self.data.write_i64::<LittleEndian>(value).unwrap();
    }

    pub fn pop_long(&mut self) -> i64 {
        let mut rdr = Cursor::new(&self.data[self.index..]);
        let value = rdr.read_i64::<LittleEndian>().unwrap();
        self.index += 8;
        value
    }

    pub fn push_float(&mut self, value: f32) {
        self.data.write_f32::<LittleEndian>(value).unwrap();
    }

    pub fn pop_float(&mut self) -> f32 {
        let mut rdr = Cursor::new(&self.data[self.index..]);
        let value = rdr.read_f32::<LittleEndian>().unwrap();
        self.index += 4;
        value
    }

    pub fn push_double(&mut self, value: f64) {
        self.data.write_f64::<LittleEndian>(value).unwrap();
    }

    pub fn pop_double(&mut self) -> f64 {
        let mut rdr = Cursor::new(&self.data[self.index..]);
        let value = rdr.read_f64::<LittleEndian>().unwrap();
        self.index += 8;
        value
    }

    pub fn push_string(&mut self, value: &str) {
        self.push_ulong(value.len() as u64);
        self.data.extend_from_slice(value.as_bytes());
    }

    pub fn pop_string(&mut self) -> String {
        let bytes = self.pop_bytes();
        String::from_utf8_lossy(&bytes).into_owned()
    }

    pub fn push_bytes(&mut self, value: &[u8]) {
        self.push_ulong(value.len() as u64);
        self.data.extend_from_slice(value);
    }

    pub fn pop_bytes(&mut self) -> Vec<u8> {
        let len = self.pop_ulong() as usize;
        if len == 0 { return Vec::new(); }
        if self.index + len > self.data.len() {
            log::error!("pop_bytes: length {} exceeds remaining buffer size", len);
            return Vec::new();
        }
        let value = self.data[self.index..self.index + len].to_vec();
        self.index += len;
        value
    }

    pub fn get_data(&self) -> &Vec<u8> {
        &self.data
    }
}

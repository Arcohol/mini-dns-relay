pub struct Message<'a> {
    pub header: Header<'a>,
    pub question: Question<'a>,
    pub answer: Answer<'a>,
}

impl<'a> Message<'a> {
    pub fn new(buf: &'a mut [u8], len: usize) -> Self {
        let (header, buf) = buf.split_at_mut(12);
        let (question, answer) = buf.split_at_mut(len - 12);

        Self {
            header: Header {
                buf: header,
                len: 12,
            },
            question: Question {
                buf: question,
                len: len - 12,
            },
            answer: Answer {
                buf: answer,
                len: 0,
            },
        }
    }

    pub fn len(&self) -> usize {
        self.header.len + self.question.len + self.answer.len
    }
}

pub struct Header<'a> {
    buf: &'a mut [u8],
    len: usize,
}

impl Header<'_> {
    pub fn get_id(&self) -> u16 {
        u16::from_be_bytes([self.buf[0], self.buf[1]])
    }

    pub fn get_qdcount(&self) -> u16 {
        u16::from_be_bytes([self.buf[4], self.buf[5]])
    }

    pub fn set_id(&mut self, id: u16) {
        self.buf[0..2].copy_from_slice(&id.to_be_bytes());
    }

    pub fn set_qr(&mut self, qr: u8) {
        self.buf[2] = (self.buf[2] & 0b0111_1111) | (qr << 7);
    }

    pub fn set_rcode(&mut self, rcode: u8) {
        self.buf[3] = (self.buf[3] & 0b1111_0000) | rcode;
    }

    pub fn set_ancount(&mut self, ancount: u16) {
        self.buf[6..8].copy_from_slice(&ancount.to_be_bytes());
    }

    pub fn set_nscount(&mut self, nscount: u16) {
        self.buf[8..10].copy_from_slice(&nscount.to_be_bytes());
    }

    pub fn set_arcount(&mut self, arcount: u16) {
        self.buf[10..12].copy_from_slice(&arcount.to_be_bytes());
    }
}

pub struct Question<'a> {
    buf: &'a [u8],
    len: usize,
}

impl Question<'_> {
    pub fn entries(&self, qdcount: u16) -> Vec<QuestionEntry> {
        let mut entries = Vec::new();
        let mut i = 0;

        for _ in 0..qdcount {
            let offset = 12 + i; // offset is calculated for later use, stored in QuestionEntry

            let mut qname = String::new();
            loop {
                let len = self.buf[i] as usize;
                if len == 0 {
                    qname.pop(); // remove the last '.'

                    i += 1; // finish reading qname, start reading qtype and qclass
                    entries.push(QuestionEntry {
                        offset,
                        qname,
                        qtype: u16::from_be_bytes([self.buf[i], self.buf[i + 1]]),
                        qclass: u16::from_be_bytes([self.buf[i + 2], self.buf[i + 3]]),
                    });

                    i += 4; // enter the next round
                    break;
                }
                qname.push_str(std::str::from_utf8(&self.buf[i + 1..=i + len]).unwrap());
                qname.push('.');

                i += len + 1;
            }
        }

        entries
    }
}

pub struct Answer<'a> {
    buf: &'a mut [u8],
    len: usize,
}

impl Answer<'_> {
    pub fn add_entries(&mut self, entries: Vec<ResourceRecord>) {
        for rr in entries {
            self.buf[self.len..self.len + 2].copy_from_slice(&rr.name.to_be_bytes());
            self.len += 2;
            self.buf[self.len..self.len + 2].copy_from_slice(&rr.rtype.to_be_bytes());
            self.len += 2;
            self.buf[self.len..self.len + 2].copy_from_slice(&rr.rclass.to_be_bytes());
            self.len += 2;
            self.buf[self.len..self.len + 4].copy_from_slice(&rr.ttl.to_be_bytes());
            self.len += 4;
            self.buf[self.len..self.len + 2].copy_from_slice(&rr.rdlength.to_be_bytes());
            self.len += 2;
            match rr.rdata {
                RData::V4(addr) => {
                    self.buf[self.len..self.len + 4].copy_from_slice(&addr);
                    self.len += 4;
                }
                RData::V6(addr) => {
                    self.buf[self.len..self.len + 16].copy_from_slice(&addr);
                    self.len += 16;
                }
            }
        }
    }
}

#[derive(Debug)]
pub struct QuestionEntry {
    pub offset: usize,
    pub qname: String,
    pub qtype: u16,
    pub qclass: u16,
}

#[derive(Debug)]
pub struct ResourceRecord {
    pub name: u16,
    pub rtype: u16,
    pub rclass: u16,
    pub ttl: u32,
    pub rdlength: u16,
    pub rdata: RData,
}

#[derive(Debug)]
pub enum RData {
    V4([u8; 4]),
    V6([u8; 16]),
}

//! 登录辅助：设备身份生成、账号 id 生成。

use rand::Rng;

pub struct DeviceIdentity {
    pub device_id: String,
    pub openudid: String,
    pub vendorid: String,
}

pub fn generate_device_identity() -> DeviceIdentity {
    DeviceIdentity {
        device_id: crate::protocol::random_hex_32(),
        openudid: uuid::Uuid::new_v4().to_string().to_uppercase(),
        vendorid: uuid::Uuid::new_v4().to_string().to_uppercase(),
    }
}

pub fn gen_account_id() -> String {
    let mut rng = rand::thread_rng();
    let bytes: Vec<u8> = (0..4).map(|_| rng.gen()).collect();
    let hex: String = bytes.iter().map(|b| format!("{:02x}", b)).collect();
    format!("acc{hex}")
}

pub fn gen_account_name(phone: &str) -> String {
    let masked = if phone.len() >= 7 {
        format!("{}****{}", &phone[..3], &phone[phone.len() - 4..])
    } else {
        phone.to_string()
    };
    format!("账号 {masked}")
}

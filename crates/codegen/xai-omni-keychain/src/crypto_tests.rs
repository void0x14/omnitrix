use super::*;

#[test]
fn roundtrip_encrypt_decrypt() {
    let salt = random_salt();
    let key = derive_key("master-pass-123", &salt, &KdfParams::default());
    let ct = encrypt(&key, b"sk-secret-key").unwrap();
    let pt = decrypt(&key, &ct).unwrap();
    assert_eq!(&*pt, b"sk-secret-key");
}

#[test]
fn wrong_password_fails() {
    let salt = random_salt();
    let k1 = derive_key("correct", &salt, &KdfParams::default());
    let k2 = derive_key("wrong", &salt, &KdfParams::default());
    let ct = encrypt(&k1, b"data").unwrap();
    assert!(decrypt(&k2, &ct).is_err());
}

#[test]
fn derive_key_is_deterministic() {
    let salt = random_salt();
    let a = derive_key("pw", &salt, &KdfParams::default());
    let b = derive_key("pw", &salt, &KdfParams::default());
    assert_eq!(*a, *b);
}

#[test]
fn different_salt_different_key() {
    let s1 = random_salt();
    let s2 = random_salt();
    let a = derive_key("pw", &s1, &KdfParams::default());
    let b = derive_key("pw", &s2, &KdfParams::default());
    assert_ne!(*a, *b);
}

#[test]
fn tampered_ciphertext_fails() {
    let salt = random_salt();
    let key = derive_key("pw", &salt, &KdfParams::default());
    let ct = encrypt(&key, b"data").unwrap();
    let raw = B64.decode(&ct).unwrap();
    let mut tampered = raw.clone();
    if let Some(b) = tampered.last_mut() {
        *b ^= 0x01;
    }
    let bad = B64.encode(tampered);
    assert!(decrypt(&key, &bad).is_err());
}

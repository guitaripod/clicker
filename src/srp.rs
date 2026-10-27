//! SRP-6a client (RFC 5054, 3072-bit group) as used by HAP pair-setup.
//!
//! Apple hashes with SHA-512 and feeds each intermediate into the next hash as
//! minimally encoded big-endian bytes, which is what the helpers below do.

use num_bigint::BigUint;
use num_traits::Zero;
use sha2::{Digest, Sha512};

const PRIME_HEX: &str = "\
FFFFFFFFFFFFFFFFC90FDAA22168C234C4C6628B80DC1CD129024E088A67CC74020BBEA6\
3B139B22514A08798E3404DDEF9519B3CD3A431B302B0A6DF25F14374FE1356D6D51C245\
E485B576625E7EC6F44C42E9A637ED6B0BFF5CB6F406B7EDEE386BFB5A899FA5AE9F2411\
7C4B1FE649286651ECE45B3DC2007CB8A163BF0598DA48361C55D39A69163FA8FD24CF5F\
83655D23DCA3AD961C62F356208552BB9ED529077096966D670C354E4ABC9804F1746C08\
CA18217C32905E462E36CE3BE39E772C180E86039B2783A2EC07A28FB5C55DF06F4C52C9\
DE2BCBF6955817183995497CEA956AE515D2261898FA051015728E5A8AAAC42DAD33170D\
04507A33A85521ABDF1CBA64ECFB850458DBEF0A8AEA71575D060C7DB3970F85A6E1E4C7\
ABF5AE8CDB0933D71E8C94E04A25619DCEE3D2261AD2EE6BF12FFA06D98A0864D8760273\
3EC86A64521F2B18177B200CBBE117577A615D6C770988C0BAD946E208E24FA074E5AB31\
43DB5BFCE0FD108E4B82D120A93AD2CAFFFFFFFFFFFFFFFF";

const GENERATOR: u64 = 5;
const USERNAME: &str = "Pair-Setup";

/// Client half of the pair-setup exchange, alive from M1 until M4.
#[derive(Debug)]
pub struct SrpClient {
    n: BigUint,
    g: BigUint,
    k: BigUint,
    a: BigUint,
    public: BigUint,
    password_hash: Vec<u8>,
    server_public: BigUint,
    session_key: Vec<u8>,
    client_proof: Vec<u8>,
}

impl SrpClient {
    pub fn new(password: &str) -> Self {
        let mut private = [0u8; 32];
        use rand::RngCore;
        rand::thread_rng().fill_bytes(&mut private);
        Self::with_private(password, &private)
    }

    pub fn with_private(password: &str, private: &[u8]) -> Self {
        let n = BigUint::parse_bytes(PRIME_HEX.as_bytes(), 16).expect("valid prime");
        let g = BigUint::from(GENERATOR);

        let mut k_input = n.to_bytes_be();
        k_input.extend_from_slice(&pad(&g, &n));
        let k = BigUint::from_bytes_be(&Sha512::digest(&k_input));

        let a = BigUint::from_bytes_be(private);
        let public = g.modpow(&a, &n);

        let password_hash = Sha512::digest(format!("{USERNAME}:{password}").as_bytes()).to_vec();

        Self {
            n,
            g,
            k,
            a,
            public,
            password_hash,
            server_public: BigUint::zero(),
            session_key: Vec::new(),
            client_proof: Vec::new(),
        }
    }

    pub fn public(&self) -> Vec<u8> {
        self.public.to_bytes_be()
    }

    /// Fold in the device's salt and public key, returning the M3 client proof.
    pub fn process(&mut self, salt: &[u8], server_public: &[u8]) -> Result<Vec<u8>, super::Error> {
        let b = BigUint::from_bytes_be(server_public);
        if b.is_zero() || (&b % &self.n).is_zero() {
            return Err(super::Error::Protocol("invalid server public key".into()));
        }
        self.server_public = b;

        let mut x_hasher = Sha512::new();
        x_hasher.update(salt);
        x_hasher.update(&self.password_hash);
        let x = BigUint::from_bytes_be(&x_hasher.finalize());

        let verifier = self.g.modpow(&x, &self.n);
        let mut u_input = pad(&self.public, &self.n);
        u_input.extend_from_slice(&pad(&self.server_public, &self.n));
        let u = BigUint::from_bytes_be(&Sha512::digest(&u_input));

        let subtract = (&self.k * &verifier) % &self.n;
        let base = if self.server_public >= subtract {
            &self.server_public - &subtract
        } else {
            (&self.server_public + &self.n - &subtract) % &self.n
        };
        let exponent = &self.a + &u * &x;
        let premaster = base.modpow(&exponent, &self.n);
        self.session_key = Sha512::digest(premaster.to_bytes_be()).to_vec();

        let mut proof_input = Vec::new();
        proof_input.extend_from_slice(&xor_digests(&self.n, &self.g));
        proof_input.extend_from_slice(&Sha512::digest(USERNAME.as_bytes()));
        proof_input.extend_from_slice(salt);
        proof_input.extend_from_slice(&self.public.to_bytes_be());
        proof_input.extend_from_slice(&self.server_public.to_bytes_be());
        proof_input.extend_from_slice(&self.session_key);
        self.client_proof = Sha512::digest(&proof_input).to_vec();

        Ok(self.client_proof.clone())
    }

    /// The device's M4 proof, so a fake Apple TV cannot harvest the PIN.
    pub fn verify_server(&self, proof: &[u8]) -> Result<(), super::Error> {
        let mut input = Vec::new();
        input.extend_from_slice(&self.public.to_bytes_be());
        input.extend_from_slice(&self.client_proof);
        input.extend_from_slice(&self.session_key);
        let expected = Sha512::digest(&input);
        if expected.as_slice() == proof {
            Ok(())
        } else {
            Err(super::Error::Authentication(
                "device failed to prove it knows the PIN".into(),
            ))
        }
    }

    /// Raw SRP session key (`K`), the input to the pairing HKDF.
    pub fn session_key(&self) -> &[u8] {
        &self.session_key
    }
}

fn pad(value: &BigUint, modulus: &BigUint) -> Vec<u8> {
    let width = modulus.to_bytes_be().len();
    let raw = value.to_bytes_be();
    let mut padded = vec![0u8; width.saturating_sub(raw.len())];
    padded.extend_from_slice(&raw);
    padded
}

fn xor_digests(n: &BigUint, g: &BigUint) -> Vec<u8> {
    let hn = Sha512::digest(n.to_bytes_be());
    let hg = Sha512::digest(g.to_bytes_be());
    hn.iter().zip(hg.iter()).map(|(a, b)| a ^ b).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    const SALT: &str = "00112233445566778899aabbccddeeff";
    const PRIVATE: &str = "1f2e3d4c5b6a798807162534435261708192a3b4c5d6e7f8091a2b3c4d5e6f70";
    const SERVER_PUBLIC: &str = "48c87c98142f176e640cc9fccd0574aaab5fecc0fba2f0fd056dfb1282aa0cb195888dab0c6ff85bf6d64f221ee3d4b8f99e995e2b1a6fde7d36cd0441153c65f2738fec9ec42a78c56533bac5bf5c34f2da2ee31a40a553c795d64c7ab3b2858ec4d3212df31872cbf89da0da56c06dc0833a424ccb306b8d090e0e909d13ae0399ecd3c4b202b1e4c0094daab4b46eeefce33c1ebe05646f4f39c04ee06d951ccf620592cc6f7bd5078851004f14b6df48fd860b53cc7130fd3bc1dac91e4fa00177fbbd519fdd112ea2f0619563ba61c819b9f29a0516c3d41922e79efc604b2e9c756f160b3ad4a044057e951b11bb6b7bdbe6631767430ca1f823acce1eb82a36e49ef0e8ee547d66ea886ad96787bb98d48f136080f1cd8219d2f34e90a45cee4928da95c834d7f0ea495d3383bbad17055e24470982097ef862a506310df3ee7c9512f13a6c08ceb6cc4317c16a5815743c8b2ddd3bc4e90543e54e2869594477f1074d4436b546e466ac62bf578cbec0d5142e3c87cd46e6856731fd";

    fn hex(s: &str) -> Vec<u8> {
        (0..s.len())
            .step_by(2)
            .map(|i| u8::from_str_radix(&s[i..i + 2], 16).unwrap())
            .collect()
    }

    fn to_hex(bytes: &[u8]) -> String {
        bytes.iter().map(|b| format!("{b:02x}")).collect()
    }

    #[test]
    fn matches_python_srptools() {
        let mut client = SrpClient::with_private("1234", &hex(PRIVATE));
        assert_eq!(
            to_hex(&client.public()),
            "c904371f483f2c3cd0b218c9346ecea627f66896555a7e3ffc892a2986158a484cfa22a091bcd03b5e8f1cdd2827f23d6b69da5a7f35239c43120790fd2ae03daf79d95a8973342f3cc1edbdbb8d854d57f64f22c9d987d2e278733e3a9eec7b8a21c38e094cc04b45a0e1685848930cbc46343ee06f3b79877ce747cc154cbed9ccf91ecca1bd0ab6348dbc9e3aabf2c24d379dc956be8a64d3b242be8f38f47a2a96d6ad9753b32947fceb4d3241af9c62b4b674d2d4fda3e3439358125cbaf3c35a116f54b3dff02c25febfeff1aaf8ec46971bb449bdd01c859f6e86abc199812a8adf0f0b90f5a6d6228d0f77791fee4ca8ce0b91e5da9c5739516b22f8526df6fc0ae4a3d681b0deeb77d21a5f647fab25f45b50c5e151240e8b95efbb5d9a6702af1368bbdb67bcec85f1de823bb5ac0b933463b714e66a544208545f6d47c265bb3aefa1c897e8cedeb743d9f41b02464fe6f08fb604acb55393fe11070b4b8dea43072d1eb3bcf16613deff1a2cd4963b7e99b272e35354abb686ce"
        );

        let proof = client
            .process(&hex(SALT), &hex(SERVER_PUBLIC))
            .expect("proof");
        assert_eq!(
            to_hex(client.session_key()),
            "c3ff9f78cdbc4a9e4dac7178d74ce9ec3fafb8209107bcdd1e22011d4dab10fc86ca761e947801973f2daf245372a8e8b556db9ddd7fa512b2a2778c5fa89f32"
        );
        assert_eq!(
            to_hex(&proof),
            "359ffb7d1bbd5de38b179dbe82b03903ad089dd5c7199796d30d3b1516a74cb9b0aedd6c53a7cc419b2ea2522810220485540adab4925a6bb97c9580f7368d4d"
        );

        client
            .verify_server(&hex("98d0014e63643d917acc65684b1ec5b01056adcf26fe4a7ea5cb3713fa9803225ff417c95db8dfd6a0219d0d4a2ae3dca66dcb5f2dbf09ef98d1919c90286626"))
            .expect("server proof");
    }

    #[test]
    fn rejects_wrong_server_proof() {
        let mut client = SrpClient::with_private("1234", &hex(PRIVATE));
        client.process(&hex(SALT), &hex(SERVER_PUBLIC)).unwrap();
        assert!(client.verify_server(&[0u8; 64]).is_err());
    }
}

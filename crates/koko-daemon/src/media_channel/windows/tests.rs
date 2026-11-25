use std::{os::windows::io::AsRawHandle, ptr};

use windows::Win32::{
    Foundation::{HANDLE, HLOCAL},
    Security::{
        ACCESS_ALLOWED_ACE, ACL,
        Authorization::{GetSecurityInfo, SE_KERNEL_OBJECT},
        DACL_SECURITY_INFORMATION, EqualSid, GetAce, OWNER_SECURITY_INFORMATION,
        PSECURITY_DESCRIPTOR, PSID,
    },
    System::SystemServices::ACCESS_ALLOWED_ACE_TYPE,
};
use windows::core::Owned;

use super::*;

#[tokio::test]
async fn pipe_owner_and_only_allowed_trustee_are_the_current_user() -> std::io::Result<()> {
    let channel = MediaChannel::bind(false)?;
    let mut descriptor = PSECURITY_DESCRIPTOR::default();
    let mut owner = PSID::default();
    let mut dacl: *mut ACL = ptr::null_mut();

    // GetSecurityInfo allocates one descriptor and returns pointers to its owner and ACL.
    // Keep that allocation live while inspecting the ACE and its inline SID.
    unsafe {
        GetSecurityInfo(
            HANDLE(channel.server.as_raw_handle()),
            SE_KERNEL_OBJECT,
            OWNER_SECURITY_INFORMATION | DACL_SECURITY_INFORMATION,
            Some(&mut owner),
            None,
            Some(&mut dacl),
            None,
            Some(&mut descriptor),
        )
        .ok()?;
        let _allocation = Owned::new(HLOCAL(descriptor.0));
        let user = current_user()?;
        let sid = user.Anonymous1.User.Sid;
        EqualSid(owner, sid)?;
        assert!(!dacl.is_null(), "a null DACL would grant everyone access");
        assert_eq!((*dacl).AceCount, 1);
        let mut ace = ptr::null_mut();
        GetAce(dacl, 0, &mut ace)?;
        let ace = ace.cast::<ACCESS_ALLOWED_ACE>();
        assert_eq!((*ace).Header.AceType as u32, ACCESS_ALLOWED_ACE_TYPE);
        EqualSid(PSID(ptr::addr_of_mut!((*ace).SidStart).cast()), sid)?;
    }
    Ok(())
}

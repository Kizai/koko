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

// On the hosted runner compare the same DACL/owner with and without other descriptor fields.
#[tokio::test]
async fn diagnose_pipe_creation_privilege() -> std::io::Result<()> {
    use windows::Win32::Security::{
        GetSecurityDescriptorControl, GetSecurityDescriptorDacl, InitializeSecurityDescriptor,
        SECURITY_DESCRIPTOR, SetSecurityDescriptorDacl, SetSecurityDescriptorOwner,
    };
    use windows::Win32::System::SystemServices::SECURITY_DESCRIPTOR_REVISION;
    use windows::core::BOOL;
    let descriptor = user_only_descriptor()?;
    let original = PSECURITY_DESCRIPTOR(descriptor.0);
    let mut flags = 0;
    let user = current_user()?;
    unsafe {
        GetSecurityDescriptorControl(original, &mut flags, &mut 0)?;
        let mut acl = ptr::null_mut();
        GetSecurityDescriptorDacl(
            original,
            &mut BOOL::default(),
            &mut acl,
            &mut BOOL::default(),
        )?;
        assert!(!acl.is_null());
        let create = |sd: PSECURITY_DESCRIPTOR| {
            let mut attributes = SECURITY_ATTRIBUTES {
                nLength: size_of::<SECURITY_ATTRIBUTES>() as u32,
                lpSecurityDescriptor: sd.0,
                bInheritHandle: false.into(),
            };
            ServerOptions::new()
                .first_pipe_instance(true)
                .max_instances(1)
                .reject_remote_clients(true)
                .create_with_security_attributes_raw(
                    format!(r"\\.\pipe\koko-video-{:032x}", GUID::new()?.to_u128()),
                    ptr::from_mut(&mut attributes).cast(),
                )
                .map(drop)
        };
        let old = create(original);
        let mut header = SECURITY_DESCRIPTOR::default();
        let bare = PSECURITY_DESCRIPTOR(ptr::from_mut(&mut header).cast());
        InitializeSecurityDescriptor(bare, SECURITY_DESCRIPTOR_REVISION)?;
        SetSecurityDescriptorDacl(bare, true, Some(acl), false)?;
        SetSecurityDescriptorControl(bare, SE_DACL_PROTECTED, SE_DACL_PROTECTED)?;
        let default_owner = create(bare);
        SetSecurityDescriptorOwner(bare, Some(user.Anonymous1.User.Sid), false)?;
        let explicit_owner = create(bare);
        assert!(
            old.is_ok(),
            "old flags={flags:#06x}, old={old:?}, explicit owner+DACL={explicit_owner:?}, DACL only={default_owner:?}"
        );
    }
    Ok(())
}

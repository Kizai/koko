use std::ptr;

use tokio::{
    io::{ReadHalf, WriteHalf, split},
    net::windows::named_pipe::{NamedPipeServer, ServerOptions},
};
use windows::{
    Win32::{
        Foundation::{GENERIC_READ, GENERIC_WRITE, HANDLE, HLOCAL},
        Security::{
            Authorization::{
                BuildTrusteeWithSidW, EXPLICIT_ACCESS_W, SET_ACCESS, SetEntriesInAclW, TRUSTEE_W,
            },
            GetTokenInformation, InitializeSecurityDescriptor, NO_INHERITANCE,
            PSECURITY_DESCRIPTOR, SE_DACL_PROTECTED, SECURITY_ATTRIBUTES, SECURITY_DESCRIPTOR,
            SetSecurityDescriptorControl, SetSecurityDescriptorDacl, SetSecurityDescriptorOwner,
            TOKEN_QUERY, TokenUser,
        },
        System::{
            SystemServices::{SE_TOKEN_USER, SECURITY_DESCRIPTOR_REVISION},
            Threading::{GetCurrentProcess, OpenProcessToken},
        },
    },
    core::{GUID, Owned},
};

use super::{MediaChannel, io};

impl MediaChannel {
    pub(crate) fn bind(_is_audio: bool) -> io::Result<Self> {
        let name = format!(r"\\.\pipe\koko-video-{:032x}", GUID::new()?.to_u128());
        let server = create_user_pipe(&name)?;
        Ok(Self { name, server })
    }

    pub(crate) async fn accept(
        &mut self,
    ) -> io::Result<(
        ReadHalf<&mut NamedPipeServer>,
        WriteHalf<&mut NamedPipeServer>,
    )> {
        self.server.connect().await?;
        Ok(split(&mut self.server))
    }
}

fn current_user() -> io::Result<Box<SE_TOKEN_USER>> {
    let mut token = HANDLE::default();
    let mut user = Box::<SE_TOKEN_USER>::default();
    // SE_TOKEN_USER holds both TOKEN_USER and its variable-length SID; the box keeps
    // the SID pointer returned by GetTokenInformation valid after this function returns.
    unsafe {
        OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token)?;
        let token = Owned::new(token);
        let mut length = 0;
        GetTokenInformation(
            *token,
            TokenUser,
            Some(ptr::from_mut(&mut *user).cast()),
            size_of::<SE_TOKEN_USER>() as u32,
            &mut length,
        )?;
    }
    Ok(user)
}

fn create_user_pipe(name: &str) -> io::Result<NamedPipeServer> {
    let user = current_user()?;
    let mut trustee = TRUSTEE_W::default();
    // This absolute descriptor borrows the token user SID and the single-entry ACL.
    // Both allocations remain live until CreateNamedPipeW returns.
    unsafe {
        let sid = user.Anonymous1.User.Sid;
        BuildTrusteeWithSidW(&mut trustee, Some(sid));
        let access = EXPLICIT_ACCESS_W {
            grfAccessPermissions: (GENERIC_READ | GENERIC_WRITE).0,
            grfAccessMode: SET_ACCESS,
            grfInheritance: NO_INHERITANCE,
            Trustee: trustee,
        };
        let mut acl = ptr::null_mut();
        SetEntriesInAclW(Some(&[access]), None, &mut acl).ok()?;
        let _acl_allocation = Owned::new(HLOCAL(acl.cast()));

        // Specify only the owner and DACL. Even an explicitly null SACL can require
        // SeSecurityPrivilege when Windows creates the pipe.
        let mut header = SECURITY_DESCRIPTOR::default();
        let descriptor = PSECURITY_DESCRIPTOR(ptr::from_mut(&mut header).cast());
        InitializeSecurityDescriptor(descriptor, SECURITY_DESCRIPTOR_REVISION)?;
        SetSecurityDescriptorOwner(descriptor, Some(sid), false)?;
        SetSecurityDescriptorDacl(descriptor, true, Some(acl), false)?;
        SetSecurityDescriptorControl(descriptor, SE_DACL_PROTECTED, SE_DACL_PROTECTED)?;
        let mut attributes = SECURITY_ATTRIBUTES {
            nLength: size_of::<SECURITY_ATTRIBUTES>() as u32,
            lpSecurityDescriptor: descriptor.0,
            bInheritHandle: false.into(),
        };
        ServerOptions::new()
            .first_pipe_instance(true)
            .max_instances(1)
            .reject_remote_clients(true)
            .create_with_security_attributes_raw(name, ptr::from_mut(&mut attributes).cast())
    }
}

#[cfg(test)]
mod tests;

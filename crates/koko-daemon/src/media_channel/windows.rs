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
                BuildSecurityDescriptorW, BuildTrusteeWithSidW, EXPLICIT_ACCESS_W, SET_ACCESS,
                TRUSTEE_W,
            },
            GetTokenInformation, NO_INHERITANCE, PSECURITY_DESCRIPTOR, SE_DACL_PROTECTED,
            SECURITY_ATTRIBUTES, SetSecurityDescriptorControl, TOKEN_QUERY, TokenUser,
        },
        System::{
            SystemServices::SE_TOKEN_USER,
            Threading::{GetCurrentProcess, OpenProcessToken},
        },
    },
    core::{GUID, Owned},
};

use super::{MediaChannel, io};

impl MediaChannel {
    pub(crate) fn bind(_is_audio: bool) -> io::Result<Self> {
        let name = format!(r"\\.\pipe\koko-video-{:032x}", GUID::new()?.to_u128());
        let descriptor = user_only_descriptor()?;
        let mut attributes = SECURITY_ATTRIBUTES {
            nLength: size_of::<SECURITY_ATTRIBUTES>() as u32,
            lpSecurityDescriptor: descriptor.0,
            bInheritHandle: false.into(),
        };
        // Tokio passes these live attributes to CreateNamedPipeW synchronously.
        let server = unsafe {
            ServerOptions::new()
                .first_pipe_instance(true)
                .max_instances(1)
                .reject_remote_clients(true)
                .create_with_security_attributes_raw(&name, ptr::from_mut(&mut attributes).cast())?
        };
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

fn user_only_descriptor() -> io::Result<Owned<HLOCAL>> {
    let user = current_user()?;
    let mut trustee = TRUSTEE_W::default();
    // BuildSecurityDescriptorW copies the trustee SID into one LocalFree-owned descriptor.
    // An explicit owner also avoids an elevated token's default Administrators owner.
    unsafe {
        BuildTrusteeWithSidW(&mut trustee, Some(user.Anonymous1.User.Sid));
        let access = EXPLICIT_ACCESS_W {
            grfAccessPermissions: (GENERIC_READ | GENERIC_WRITE).0,
            grfAccessMode: SET_ACCESS,
            grfInheritance: NO_INHERITANCE,
            Trustee: trustee,
        };
        let mut size = 0;
        let mut descriptor = PSECURITY_DESCRIPTOR::default();
        BuildSecurityDescriptorW(
            Some(&trustee),
            None,
            Some(&[access]),
            None,
            None,
            &mut size,
            &mut descriptor,
        )
        .ok()?;
        let allocation = Owned::new(HLOCAL(descriptor.0));
        SetSecurityDescriptorControl(descriptor, SE_DACL_PROTECTED, SE_DACL_PROTECTED)?;
        Ok(allocation)
    }
}

#[cfg(test)]
mod tests;

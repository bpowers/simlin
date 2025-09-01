import { ImageList, ImageListItem, Paper, Typography } from '@mui/material';
import Link from 'next/link';

import getSignedInUser from '@/lib/getSignedInUser';
import { getProjects } from '@/lib/database/projects';

export default async function Home() {
  const user = await getSignedInUser();

  const projects = await getProjects(user.uid);

  return (
    <ImageList cols={2} gap={0}>
      {projects.map((project) => (
        <ImageListItem key={project.id}>
          <Link href={`/${project.id}`} className="simlin-home-modellink">
            <Paper className="simlin-home-paper" elevation={4}>
              <div className="simlin-home-preview">
                <img src={`/api/preview/${project.id}`} alt="model preview" />
              </div>
              <Typography variant="h5" component="h3">
                {project.displayName}
              </Typography>
              <Typography component="p">{project.description}</Typography>
            </Paper>
          </Link>
        </ImageListItem>
      ))}
    </ImageList>
  );
}
